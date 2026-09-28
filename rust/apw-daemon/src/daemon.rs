use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use apw_core::{append_jsonl, utc_timestamp, Ed25519Signer, ProofLevel, DEFAULT_EVIDENCE_BACKUPS, DEFAULT_EVIDENCE_MAX_BYTES};
use serde_json::{json, Value};

use crate::assembly::{generate_manifest, AssemblyContext, AssemblyInputs, GeneratedManifest, ManifestServices};
use crate::correlation::{CorrelationEngine, LayerEvent};
use crate::coverage::{derive_coverage, CoverageInputs};
use crate::error::{DaemonError, Result};
use crate::probe::{AudioProbe, UnavailableAudioProbe};
use crate::receiver::{EvidenceReceiver, MAX_DATAGRAM_BYTES};
use crate::services::{
    ClaimIssuer, ExportAssociator, ForgeryAnalyzer, LocalSealer, TimeAnchor, UnavailableAssociator,
    UnavailableClaimIssuer, UnavailableForgeryAnalysis, UnsealedManifest,
};
use crate::session::{event_type_to_layer, session_diagnostics, SessionState};
use crate::watcher::{
    DetectedExport, ExportWatcher, SampleWatcher, StabilityPolicy, ThreadSleep,
};

pub const DEFAULT_UDP_PORT: u16 = 9876;

/// Producer-declared origin of the routed stem. `Unknown` is the only value the
/// daemon can assert on its own; everything else is `user_declared`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCategory {
    Unknown,
    AudioInterfaceRecording,
    MidiVstSynth,
    ImportedSample,
    Generator,
    Resampling,
    ManualImport,
}

impl SourceCategory {
    pub const ALL: [SourceCategory; 7] = [
        SourceCategory::Unknown,
        SourceCategory::AudioInterfaceRecording,
        SourceCategory::MidiVstSynth,
        SourceCategory::ImportedSample,
        SourceCategory::Generator,
        SourceCategory::Resampling,
        SourceCategory::ManualImport,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceCategory::Unknown => "unknown",
            SourceCategory::AudioInterfaceRecording => "audio_interface_recording",
            SourceCategory::MidiVstSynth => "midi_vst_synth",
            SourceCategory::ImportedSample => "imported_sample",
            SourceCategory::Generator => "generator",
            SourceCategory::Resampling => "resampling",
            SourceCategory::ManualImport => "manual_import",
        }
    }

    pub fn parse(value: &str) -> Option<SourceCategory> {
        SourceCategory::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    /// A declared category is the producer's word, never an observation.
    pub fn proof_level(self) -> ProofLevel {
        match self {
            SourceCategory::Unknown => ProofLevel::UnknownUnobserved,
            _ => ProofLevel::UserDeclared,
        }
    }
}

pub struct DaemonConfig {
    pub udp_host: String,
    pub udp_port: u16,
    pub evidence_dir: PathBuf,
    pub manifest_dir: PathBuf,
    pub sample_dir: PathBuf,
    pub export_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    pub stem_id: String,
    pub source_category: SourceCategory,
    pub generate_html_report: bool,
    pub poll_interval: Duration,
    pub export_stability: StabilityPolicy,
    pub sample_stable_polls: u32,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            udp_host: "127.0.0.1".to_owned(),
            udp_port: DEFAULT_UDP_PORT,
            evidence_dir: PathBuf::from("evidence"),
            manifest_dir: PathBuf::from("manifests"),
            sample_dir: PathBuf::from("samples"),
            export_dir: None,
            session_id: None,
            stem_id: "stem-1".to_owned(),
            source_category: SourceCategory::Unknown,
            generate_html_report: true,
            poll_interval: Duration::from_secs(2),
            export_stability: StabilityPolicy::default(),
            sample_stable_polls: 2,
        }
    }
}

/// The collaborating engines the daemon does not own. Every default is the
/// honest "not configured" implementation, so a daemon assembled without the
/// audio or C2PA crates still produces a manifest that says exactly what it
/// observed and claims nothing it did not.
pub struct DaemonServices {
    pub audio: Arc<dyn AudioProbe>,
    pub associator: Arc<dyn ExportAssociator>,
    pub forgery: Arc<dyn ForgeryAnalyzer>,
    pub claims: Arc<dyn ClaimIssuer>,
    pub sealer: Arc<dyn LocalSealer>,
    pub portable_signer: Option<Arc<Ed25519Signer>>,
    pub time_anchor: Option<Arc<dyn TimeAnchor>>,
}

impl Default for DaemonServices {
    fn default() -> Self {
        DaemonServices {
            audio: Arc::new(UnavailableAudioProbe),
            associator: Arc::new(UnavailableAssociator),
            forgery: Arc::new(UnavailableForgeryAnalysis),
            claims: Arc::new(UnavailableClaimIssuer),
            sealer: Arc::new(UnsealedManifest),
            portable_signer: None,
            time_anchor: None,
        }
    }
}

/// Interruptible sleep shared by every worker loop.
#[derive(Debug, Default)]
pub struct StopSignal {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl StopSignal {
    pub fn stop(&self) {
        if let Ok(mut stopped) = self.stopped.lock() {
            *stopped = true;
        }
        self.changed.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.lock().map(|stopped| *stopped).unwrap_or(true)
    }

    /// Returns `true` when the daemon was asked to stop during the wait.
    pub fn wait(&self, timeout: Duration) -> bool {
        let Ok(stopped) = self.stopped.lock() else {
            return true;
        };
        if *stopped {
            return true;
        }
        match self.changed.wait_timeout(stopped, timeout) {
            Ok((stopped, _)) => *stopped,
            Err(_) => true,
        }
    }
}

#[derive(Debug, Default)]
struct SealedState {
    previous_cosignature_hash: String,
    last_manifest_path: Option<PathBuf>,
    last_export_path: Option<PathBuf>,
}

/// Runtime and assembly.
///
/// LOCK ORDER, outermost first: `session` -> `sealed`. The receiver and the
/// correlation engine own their own leaf locks and are never called while either
/// of these is held, and no lock is ever held across filesystem or socket I/O.
pub struct Daemon {
    config: DaemonConfig,
    services: DaemonServices,
    session_id: String,
    session_started_at: String,
    receiver: EvidenceReceiver,
    correlation: CorrelationEngine,
    session: Mutex<SessionState>,
    sealed: Mutex<SealedState>,
    stop: Arc<StopSignal>,
}

impl Daemon {
    pub fn new(config: DaemonConfig, services: DaemonServices) -> Result<Daemon> {
        std::fs::create_dir_all(&config.evidence_dir)
            .map_err(|source| DaemonError::io("create", &config.evidence_dir, source))?;
        std::fs::create_dir_all(&config.manifest_dir)
            .map_err(|source| DaemonError::io("create", &config.manifest_dir, source))?;

        let session_id = config.session_id.clone().unwrap_or_else(|| {
            format!(
                "capture-{}-{}",
                apw_core::utc_timestamp_seconds(None).replace(['-', ':'], ""),
                crate::util::random_hex(4)
            )
        });
        let receiver = EvidenceReceiver::bind(
            &config.udp_host,
            config.udp_port,
            &config.evidence_dir.join("plugin_events.jsonl"),
            Some(session_id.clone()),
            Some(config.stem_id.clone()),
        )?;
        let correlation =
            CorrelationEngine::new(2000, &config.evidence_dir.join("composite_events.jsonl"));

        Ok(Daemon {
            session_id,
            session_started_at: utc_timestamp(None),
            receiver,
            correlation,
            session: Mutex::new(SessionState::new()),
            sealed: Mutex::new(SealedState {
                // Cosignature chain across this session's manifests: each
                // manifest_signature entangles the previous one.
                previous_cosignature_hash: "genesis".to_owned(),
                ..SealedState::default()
            }),
            stop: Arc::new(StopSignal::default()),
            config,
            services,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn receiver(&self) -> &EvidenceReceiver {
        &self.receiver
    }

    pub fn correlation(&self) -> &CorrelationEngine {
        &self.correlation
    }

    pub fn stop_signal(&self) -> Arc<StopSignal> {
        Arc::clone(&self.stop)
    }

    pub fn stop(&self) {
        self.stop.stop();
    }

    /// Clears the accumulated observation state for a new take.
    ///
    /// IMPORTANT: every counter coverage compares must restart together. The
    /// session's `chain_length` and the receiver's `events_received` are graded
    /// against each other, so resetting one side alone would make
    /// `complete_observed_path` unreachable for the rest of the process.
    ///
    /// Identity-bearing state (session id, receiver instance id, cosignature
    /// chain) belongs to the daemon instance, exactly as in the Python daemon
    /// where a new session means a new process: a reset take must not inherit an
    /// identifier that a previous take's evidence already committed to. The
    /// plug-in's own cumulative counters do not restart either, which is why a
    /// reset take whose plug-in instance predates it grades partial until the
    /// device is re-added.
    pub fn reset_session_state(&self) {
        if let Ok(mut session) = self.session.lock() {
            session.reset();
        }
        self.correlation.reset();
        self.receiver.reset();
    }

    /// Blocking run loop: the UDP intake, the sample watcher and (when an export
    /// directory is configured) the export watcher, joined on stop.
    pub fn run(&self) -> Result<()> {
        self.write_evidence(
            "session_events.jsonl",
            &json!({
                "event_type": "session_start",
                "capture_session_id": self.session_id,
                "stem_id": self.config.stem_id,
                "started_at": self.session_started_at,
                "daemon_monotonic_ms": crate::util::monotonic_millis(),
                "proof_level": ProofLevel::DirectlyObserved.as_str(),
            }),
        );

        let outcome = std::thread::scope(|scope| -> Result<()> {
            scope.spawn(|| self.run_udp_receiver());
            scope.spawn(|| self.run_sample_watcher());
            if self.config.export_dir.is_some() {
                scope.spawn(|| self.run_export_watcher());
            }
            while !self.stop.wait(Duration::from_secs(1)) {}
            Ok(())
        });

        self.write_evidence(
            "session_events.jsonl",
            &json!({
                "event_type": "session_end",
                "capture_session_id": self.session_id,
                "ended_at": utc_timestamp(None),
                "daemon_monotonic_ms": crate::util::monotonic_millis(),
                "proof_level": ProofLevel::DirectlyObserved.as_str(),
                "diagnostics": self.receiver.diagnostics().to_json(),
            }),
        );
        outcome
    }

    fn run_udp_receiver(&self) {
        if let Err(error) = self.receiver.set_read_timeout(Some(Duration::from_secs(1))) {
            log::error!("Could not set the evidence socket timeout: {error}");
            return;
        }
        // One buffer for the life of the thread: a flood cannot make the intake
        // allocate, and an oversize datagram is truncated by the socket.
        let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
        while !self.stop.is_stopped() {
            let (length, address) = match self.receiver.recv_into(&mut buffer) {
                Ok(received) => received,
                Err(error) => {
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) {
                        continue;
                    }
                    if self.stop.is_stopped() {
                        break;
                    }
                    log::warn!("UDP socket error: {error}");
                    continue;
                }
            };
            let Some(datagram) = buffer.get(..length) else {
                continue;
            };
            let outcome = self.accept_datagram(datagram);
            if !self
                .receiver
                .send_acknowledgement(address, &outcome.acknowledgement)
            {
                log::warn!("Could not dispatch local daemon acknowledgement to {address}");
            }
        }
    }

    /// The whole intake path for one datagram: validate, persist, record into the
    /// session, correlate. Public because it is the seam the in-process host (the
    /// plug-in linking this crate as a static library) feeds instead of a socket,
    /// and because the socket loop must not be the only way to reach it.
    ///
    /// The caller dispatches the returned acknowledgement; no lock is held while
    /// the evidence write or the acknowledgement send happens.
    pub fn accept_datagram(&self, data: &[u8]) -> crate::receiver::PacketOutcome {
        let outcome = self.receiver.ingest(data);
        let Some(event) = outcome.event.clone() else {
            return outcome;
        };
        let event_type = outcome
            .event_type
            .map(|kind| kind.as_str())
            .unwrap_or_default();
        let layer = event_type_to_layer(event_type);
        if let Ok(mut session) = self.session.lock() {
            session.record_plugin_event(&event, layer);
        }
        let timestamp_ms = event
            .get("daemon_received_monotonic_ms")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        self.correlate(LayerEvent::new(layer, event_type, timestamp_ms, event));
        outcome
    }

    /// The live `observation_coverage` record, from the same counters the sealed
    /// manifest grades.
    pub fn coverage(&self) -> Value {
        let receiver = self.receiver.diagnostics();
        let (telemetry, plugin_instance_count, chain_length, feature_window_drops, regressions) =
            match self.session.lock() {
                Ok(session) => (
                    session.telemetry(),
                    session.plugin_instance_count(),
                    session.chain_length(),
                    session.feature_window_drops(),
                    session.telemetry_regressions(),
                ),
                Err(_) => (Vec::new(), 0, 0, 0, 0),
            };
        derive_coverage(&CoverageInputs {
            telemetry: &telemetry,
            receiver: &receiver,
            plugin_instance_count,
            chain_length,
            feature_window_drops,
            telemetry_regressions: regressions,
        })
    }

    fn correlate(&self, layer_event: LayerEvent) {
        for composite in self.correlation.ingest(layer_event) {
            log::info!(
                "Composite edit: {} ({:.2})",
                composite.edit_type,
                composite.confidence
            );
            if let Ok(mut session) = self.session.lock() {
                session.append_event(composite.to_event());
            }
        }
    }

    fn run_sample_watcher(&self) {
        let mut watcher = SampleWatcher::new(
            &self.config.sample_dir,
            &self.config.evidence_dir.join("sample_import_events.jsonl"),
            self.config.poll_interval,
            self.config.sample_stable_polls,
            false,
        );
        if let Err(error) = watcher.mark_existing_seen() {
            log::warn!("Sample watcher could not read its watch directory: {error}");
        }
        while !self.stop.is_stopped() {
            match watcher.scan_once(self.services.audio.as_ref()) {
                Ok(events) => {
                    for mut event in events {
                        let observed_at = event.get("observed_at").cloned().unwrap_or(Value::Null);
                        let monotonic = crate::util::monotonic_millis();
                        if let Some(map) = event.as_object_mut() {
                            map.insert("source_timestamp".to_owned(), observed_at);
                            map.insert(
                                "daemon_observed_monotonic_ms".to_owned(),
                                json!(monotonic),
                            );
                        }
                        if let Ok(mut session) = self.session.lock() {
                            session.mark_layer_active("sample_watcher");
                            session.append_event(event.clone());
                        }
                        self.correlate(LayerEvent::new(
                            "sample_watcher",
                            "sample_file_observed",
                            monotonic,
                            event,
                        ));
                    }
                }
                Err(error) => log::warn!("Sample watcher error: {error}"),
            }
            if self.stop.wait(self.config.poll_interval) {
                break;
            }
        }
    }

    fn run_export_watcher(&self) {
        let Some(export_dir) = &self.config.export_dir else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(export_dir) {
            log::error!("Export watcher could not create {}: {error}", export_dir.display());
            return;
        }
        let mut watcher = ExportWatcher::new(export_dir, self.config.export_stability);
        watcher.mark_existing_seen();
        let delay = ThreadSleep;
        while !self.stop.is_stopped() {
            for export in watcher.detect(&delay) {
                log::info!("Export detected: {}", export.path.display());
                match self.seal_export(&export) {
                    Ok(generated) => {
                        log::info!("Manifest written: {}", generated.manifest_path.display());
                        watcher.commit(&export);
                    }
                    Err(error) => log::error!(
                        "Manifest generation failed for {}; will retry: {error}",
                        export.path.display()
                    ),
                }
            }
            if self.stop.wait(self.config.poll_interval) {
                break;
            }
        }
    }

    /// Hashes, assembles, signs and writes the manifest for one stable export.
    pub fn seal_export(&self, export: &DetectedExport) -> Result<GeneratedManifest> {
        let receiver_diagnostics = self.receiver.diagnostics();
        let (snapshot, plugin_instance_count) = {
            let session = self
                .session
                .lock()
                .map_err(|_| poisoned("session state"))?;
            (session.snapshot(), session.plugin_instance_count())
        };
        let coverage = derive_coverage(&CoverageInputs {
            telemetry: &snapshot.telemetry,
            receiver: &receiver_diagnostics,
            plugin_instance_count,
            chain_length: snapshot.chain_length,
            feature_window_drops: snapshot.feature_window_drops,
            telemetry_regressions: snapshot.telemetry_regressions,
        });
        let receipt_summary = self.receiver.receipt_summary();
        let diagnostics = session_diagnostics(
            &receiver_diagnostics,
            self.correlation.diagnostics().to_json(),
            json!({
                "events_retained": snapshot.events_retained,
                "max_events": crate::session::MAX_SESSION_EVENTS,
                "events_dropped_from_memory_only": snapshot.events_dropped,
            }),
            receipt_summary.clone(),
        );

        let previous_cosignature_hash = self
            .sealed
            .lock()
            .map(|sealed| sealed.previous_cosignature_hash.clone())
            .unwrap_or_else(|_| "genesis".to_owned());

        let context = AssemblyContext {
            session_id: &self.session_id,
            stem_id: &self.config.stem_id,
            source_category: self.config.source_category.as_str(),
            source_category_proof_level: self.config.source_category.proof_level(),
            evidence_dir: &self.config.evidence_dir,
            manifest_dir: &self.config.manifest_dir,
            session_started_at: &self.session_started_at,
            generate_html_report: self.config.generate_html_report,
            previous_cosignature_hash: &previous_cosignature_hash,
        };
        let services = ManifestServices {
            audio: self.services.audio.as_ref(),
            associator: self.services.associator.as_ref(),
            forgery: self.services.forgery.as_ref(),
            claims: self.services.claims.as_ref(),
            sealer: self.services.sealer.as_ref(),
            portable_signer: self.services.portable_signer.as_deref(),
            time_anchor: self.services.time_anchor.as_deref(),
        };
        let inputs = AssemblyInputs {
            snapshot: &snapshot,
            coverage,
            session_diagnostics: diagnostics,
            receipt_summary,
            session_facts: None,
            project_sample_refs: Vec::new(),
        };
        let generated = generate_manifest(
            &context,
            &services,
            &inputs,
            &export.path,
            export.version,
        )?;

        if let Ok(mut sealed) = self.sealed.lock() {
            if let Some(hash) = &generated.cosignature_hash {
                sealed.previous_cosignature_hash = hash.clone();
            }
            sealed.last_manifest_path = Some(generated.manifest_path.clone());
            sealed.last_export_path = Some(export.path.clone());
        }
        Ok(generated)
    }

    pub fn last_manifest_path(&self) -> Option<PathBuf> {
        self.sealed
            .lock()
            .ok()
            .and_then(|sealed| sealed.last_manifest_path.clone())
    }

    fn write_evidence(&self, file_name: &str, record: &Value) {
        let path = self.config.evidence_dir.join(file_name);
        if let Err(error) = append_jsonl(
            &path,
            record,
            DEFAULT_EVIDENCE_MAX_BYTES,
            DEFAULT_EVIDENCE_BACKUPS,
        ) {
            log::error!("Could not append {}: {error}", path.display());
        }
    }
}

fn poisoned(what: &'static str) -> DaemonError {
    DaemonError::io(
        "lock",
        Path::new(what),
        std::io::Error::other("lock poisoned by a panicking thread"),
    )
}
