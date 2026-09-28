//! Evidence receiver, session state, export detection and manifest assembly for
//! the audio provenance daemon.
//!
//! The crate owns the untrusted UDP boundary, the bounded session state behind
//! it, the filesystem watchers, and the assembly sequence that turns a detected
//! export into a signed manifest. Engines it does not own (container parsing,
//! export association, forgery screening, C2PA claim signing, local sealing) are
//! traits with an honest "not configured" default, so a manifest never claims an
//! observation that no engine made.

mod anchor;
mod assembly;
mod correlation;
mod coverage;
mod daemon;
mod error;
mod forgery;
mod probe;
mod receiver;
mod services;
mod session;
mod taxonomy;
mod util;
mod watcher;

pub use anchor::{HttpTransport, Rfc3161Anchor, TsaTransport};
pub use assembly::{
    generate_manifest, AssemblyContext, AssemblyInputs, GeneratedManifest, ManifestServices,
    ALL_LAYERS,
};
pub use correlation::{
    default_rules, ArrangementEditRule, AutomationEditRule, ClipDeleteRule, ClipPasteRule,
    CompositeEdit, ContentChangeRule, CorrelationCandidate, CorrelationDiagnostics,
    CorrelationEngine, CorrelationRule, DeviceAddedRule, EffectChangeRule, LayerEvent,
    MidiEditRule, ParameterAdjustRule, RecordingStartRule, SampleImportRule, UndoRule,
};
pub use coverage::{derive_coverage, CoverageInputs};
pub use daemon::{
    Daemon, DaemonConfig, DaemonServices, SourceCategory, StopSignal, DEFAULT_UDP_PORT,
};
pub use error::{DaemonError, Result};
pub use forgery::{
    derive_forgery_analysis, AudioStreamAnalyzer, ForgeryFlag, ForgeryReport, HashChainAnalyzer,
    InputBehaviorAnalyzer, StatisticalForgeryScreen,
};
pub use probe::{AudioFingerprint, AudioMetadata, AudioProbe, UnavailableAudioProbe};
pub use receiver::{
    EvidenceReceiver, PacketOutcome, ReceiverDiagnostics, StreamReceiptState, ACK_PROTOCOL,
    MAX_DATAGRAM_BYTES, MAX_TRACKED_STREAMS,
};
pub use services::{
    unavailable_association, ClaimIssuer, ClaimRequest, ExportAssociator, ForgeryAnalyzer,
    LocalSealer, Seal, TimeAnchor, UnavailableAssociator, UnavailableClaimIssuer,
    UnavailableForgeryAnalysis, UnsealedManifest, ASSOCIATION_METHOD, ASSOCIATION_METHOD_VERSION,
};
pub use session::{
    event_type_to_layer, session_diagnostics, SessionSnapshot, SessionState, MAX_FEATURE_WINDOWS,
    MAX_SESSION_EVENTS, MAX_TRACKED_PLUGIN_KEYS,
};
pub use taxonomy::{validate_event, validate_network_event, EventType};
pub use util::{monotonic_millis, python_round};
pub use watcher::{
    audio_files, build_sample_file_event, Delay, DetectedExport, ExportWatcher, FileSignature,
    SampleWatcher, StabilityPolicy, ThreadSleep, AUDIO_EXTENSIONS, DEFAULT_SAMPLE_NOTES,
    EXPORT_EXTENSIONS,
};
