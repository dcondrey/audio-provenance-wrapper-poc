use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apw_core::{
    append_jsonl, canonical_json_utf8, utc_timestamp, DEFAULT_EVIDENCE_BACKUPS,
    DEFAULT_EVIDENCE_MAX_BYTES,
};
use serde_json::{json, Map, Value};

use crate::error::{DaemonError, Result};
use crate::taxonomy::{validate_network_event, EventType};
use crate::util::random_hex;

/// Stream keys are sender-controlled; the table LRU-evicts at capacity rather
/// than refusing new streams, so the live subject is never locked out once stale
/// per-reinstantiation keys fill it. Eviction is not a silent reset: any eviction
/// permanently degrades the receipt, and a returning evicted stream's chain gap
/// is recorded as a break.
pub const MAX_TRACKED_STREAMS: usize = 64;

/// A datagram larger than this is truncated by the socket itself; the buffer is
/// allocated once per receiver, never per packet, so a flood cannot size it.
pub const MAX_DATAGRAM_BYTES: usize = 65_535;

pub const ACK_PROTOCOL: &str = "apw-local-udp-ack-v1";

const ACK_OPERATIONAL_SCOPE: &str =
    "Local daemon validation and persistence receipt only; not identity proof, \
     remote attestation, DAW trust, or registry confirmation.";

const RECEIPT_SCOPE: &str =
    "The daemon directly observed validation, persistence, and local acknowledgement \
     dispatch. It cannot observe whether every UDP acknowledgement reached the plug-in UI.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamReceiptState {
    pub highest_accepted_sequence: i64,
    pub highest_contiguous_sequence: i64,
    pub gaps: u64,
    pub rejections: u64,
    pub chain_breaks: u64,
    pub last_window_hash: Option<String>,
}

type StreamKey = (String, String);

#[derive(Debug, Default)]
struct StreamTable {
    /// Least-recently-active first. Bounded at [`MAX_TRACKED_STREAMS`], so the
    /// linear scan is over at most 64 entries.
    entries: Vec<(StreamKey, StreamReceiptState)>,
    evictions: u64,
}

impl StreamTable {
    fn position(&self, key: &StreamKey) -> Option<usize> {
        self.entries.iter().position(|(existing, _)| existing == key)
    }

    fn peek(&self, key: &StreamKey) -> Option<&StreamReceiptState> {
        self.position(key)
            .and_then(|index| self.entries.get(index))
            .map(|(_, state)| state)
    }

    /// Returns the index of the stream's state, evicting the least-recently-active
    /// stream at capacity.
    fn track(&mut self, key: &StreamKey) -> usize {
        if let Some(index) = self.position(key) {
            let entry = self.entries.remove(index);
            self.entries.push(entry);
        } else {
            self.entries.push((key.clone(), StreamReceiptState::default()));
            if self.entries.len() > MAX_TRACKED_STREAMS {
                self.entries.remove(0);
                self.evictions = self.evictions.saturating_add(1);
            }
        }
        self.entries.len().saturating_sub(1)
    }

    fn state_at(&mut self, index: usize) -> Option<&mut StreamReceiptState> {
        self.entries.get_mut(index).map(|(_, state)| state)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReceiverDiagnostics {
    pub packets_received: u64,
    pub events_received: u64,
    pub events_rejected: u64,
    pub sequence_gaps: u64,
    pub events_missing_sequence: u64,
    pub sequence_out_of_order: u64,
    pub hash_chain_breaks: u64,
    pub stream_evictions: u64,
    pub acknowledgements_attempted: u64,
    pub acknowledgements_sent: u64,
    pub acknowledgements_failed: u64,
    pub evidence_writes_failed: u64,
}

impl ReceiverDiagnostics {
    /// Insertion order matches `EvidenceReceiver.diagnostics()`; the map is copied
    /// verbatim into `observation_coverage.counters`, which is signed.
    pub fn to_json(&self) -> Value {
        json!({
            "packets_received": self.packets_received,
            "events_received": self.events_received,
            "events_rejected": self.events_rejected,
            "sequence_gaps": self.sequence_gaps,
            "events_missing_sequence": self.events_missing_sequence,
            "sequence_out_of_order": self.sequence_out_of_order,
            "hash_chain_breaks": self.hash_chain_breaks,
            "stream_evictions": self.stream_evictions,
            "daemon_acknowledgements_attempted": self.acknowledgements_attempted,
            "daemon_acknowledgements_sent": self.acknowledgements_sent,
            "daemon_acknowledgements_failed": self.acknowledgements_failed,
        })
    }
}

#[derive(Debug, Default)]
struct ReceiverState {
    counters: ReceiverDiagnostics,
    streams: StreamTable,
}

/// One accepted or rejected datagram.
#[derive(Debug, Clone)]
pub struct PacketOutcome {
    /// The enriched event, absent when the datagram was rejected.
    pub event: Option<Value>,
    pub event_type: Option<EventType>,
    pub acknowledgement: Value,
}

/// Bounded UDP evidence intake.
///
/// LOCK ORDER: `state` is a leaf. No socket, filesystem, or logging-with-format
/// work happens while it is held, and no other lock in this crate is acquired
/// under it.
///
/// IMPORTANT: [`EvidenceReceiver::ingest`] is single-consumer. The Python
/// receiver runs on one thread and increments `event_count` only after the
/// evidence write succeeds; keeping the write outside the lock preserves that
/// ordering for one reader thread while still letting other threads snapshot
/// counters. Two concurrent ingest calls would interleave evidence lines.
pub struct EvidenceReceiver {
    host: String,
    port: u16,
    evidence_path: PathBuf,
    capture_session_id: Option<String>,
    stem_id: Option<String>,
    socket: UdpSocket,
    receiver_instance_id: String,
    evidence_max_bytes: u64,
    evidence_backups: u32,
    state: Mutex<ReceiverState>,
}

impl EvidenceReceiver {
    pub fn bind(
        host: &str,
        port: u16,
        evidence_path: &Path,
        capture_session_id: Option<String>,
        stem_id: Option<String>,
    ) -> Result<EvidenceReceiver> {
        let address = format!("{host}:{port}");
        let socket = UdpSocket::bind(&address).map_err(|source| DaemonError::Bind {
            address: address.clone(),
            source,
        })?;
        Ok(EvidenceReceiver {
            host: host.to_owned(),
            port,
            evidence_path: evidence_path.to_path_buf(),
            capture_session_id,
            stem_id,
            socket,
            receiver_instance_id: format!("receiver-{}", random_hex(6)),
            evidence_max_bytes: DEFAULT_EVIDENCE_MAX_BYTES,
            evidence_backups: DEFAULT_EVIDENCE_BACKUPS,
            state: Mutex::new(ReceiverState::default()),
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn evidence_path(&self) -> &Path {
        &self.evidence_path
    }

    pub fn instance_id(&self) -> &str {
        &self.receiver_instance_id
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        self.socket
            .set_read_timeout(timeout)
            .map_err(|source| DaemonError::io("configure the evidence socket", &self.evidence_path, source))
    }

    pub fn recv_into(&self, buffer: &mut [u8; MAX_DATAGRAM_BYTES]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }

    /// Validate, enrich and persist one datagram. The evidence write happens with
    /// no lock held.
    pub fn ingest(&self, data: &[u8]) -> PacketOutcome {
        let mut outcome = self.process_packet(data);
        if let Some(event) = &outcome.event {
            if let Err(error) = append_jsonl(
                &self.evidence_path,
                event,
                self.evidence_max_bytes,
                self.evidence_backups,
            ) {
                log::error!("Could not append plug-in evidence: {error}");
                if let Ok(mut state) = self.state.lock() {
                    state.counters.evidence_writes_failed =
                        state.counters.evidence_writes_failed.saturating_add(1);
                }
                // IMPORTANT: the acknowledgement is a persistence receipt. An
                // event that was validated but never reached the evidence log
                // must not be acknowledged as accepted.
                if let Some(ack) = outcome.acknowledgement.as_object_mut() {
                    ack.insert("accepted".to_owned(), Value::Bool(false));
                    ack.insert(
                        "receipt_state".to_owned(),
                        json!("rejected_not_persisted"),
                    );
                    ack.insert(
                        "reason".to_owned(),
                        json!(format!("evidence could not be persisted: {error}")),
                    );
                }
                return PacketOutcome {
                    event: None,
                    event_type: outcome.event_type,
                    acknowledgement: outcome.acknowledgement,
                };
            }
            if let Ok(mut state) = self.state.lock() {
                state.counters.events_received = state.counters.events_received.saturating_add(1);
            }
        }
        outcome
    }

    /// Pure validation and enrichment: no I/O, and `events_received` is only
    /// committed by [`EvidenceReceiver::ingest`] once the record is durable.
    pub fn process_packet(&self, data: &[u8]) -> PacketOutcome {
        let mut rejections: Vec<String> = Vec::new();
        let outcome = {
            let Ok(mut state) = self.state.lock() else {
                return PacketOutcome {
                    event: None,
                    event_type: None,
                    acknowledgement: json!({}),
                };
            };
            self.process_locked(&mut state, data, &mut rejections)
        };
        for reason in rejections {
            log::warn!("Rejected UDP event: {reason}");
        }
        outcome
    }

    fn process_locked(
        &self,
        state: &mut ReceiverState,
        data: &[u8],
        rejections: &mut Vec<String>,
    ) -> PacketOutcome {
        state.counters.packets_received = state.counters.packets_received.saturating_add(1);

        let empty = Map::new();
        let decoded: Option<Map<String, Value>> = std::str::from_utf8(data)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            });
        let Some(mut event) = decoded else {
            // A pathologically nested datagram must reject, not kill the thread:
            // serde_json enforces its own recursion limit and returns an error.
            let reason = "invalid JSON/UTF-8 object";
            Self::reject(state, reason, rejections);
            let ack = self.build_ack(state, &empty, false, "rejected_invalid", Some(reason), None);
            return PacketOutcome {
                event: None,
                event_type: None,
                acknowledgement: ack,
            };
        };

        let instance_id = fallback_str(&event, "plugin_instance_id", "unknown_plugin_instance");
        let plugin_session_id = fallback_str(
            &event,
            "plugin_capture_session_id",
            "unknown_plugin_capture_session",
        );
        let stream_key: StreamKey = (instance_id.clone(), plugin_session_id.clone());

        let event_type = match validate_network_event(&event) {
            Ok(event_type) => event_type,
            Err(reason) => {
                Self::reject(state, &reason, rejections);
                let index = state.streams.position(&stream_key);
                if let Some(index) = index {
                    if let Some(known) = state.streams.state_at(index) {
                        known.rejections = known.rejections.saturating_add(1);
                    }
                }
                let known = index.and_then(|index| state.streams.entries.get(index)).map(|(_, s)| s.clone());
                let ack = self.build_ack(
                    state,
                    &event,
                    false,
                    "rejected_invalid",
                    Some(&reason),
                    known,
                );
                return PacketOutcome {
                    event: None,
                    event_type: None,
                    acknowledgement: ack,
                };
            }
        };

        let index = state.streams.track(&stream_key);

        let now = SystemTime::now();
        let received_at_ms = epoch_millis(now);
        let received_monotonic_ms = crate::util::monotonic_millis();
        let source_timestamp_ms = event.get("timestamp_ms").cloned().unwrap_or(Value::Null);
        event.insert("source_timestamp_ms".to_owned(), source_timestamp_ms);
        event.insert(
            "received_at".to_owned(),
            json!(utc_timestamp(Some(
                UNIX_EPOCH + Duration::from_millis(received_at_ms.max(0) as u64)
            ))),
        );
        event.insert("received_at_ms".to_owned(), json!(received_at_ms));
        event.insert(
            "daemon_received_monotonic_ms".to_owned(),
            json!(received_monotonic_ms),
        );
        if let Some(session) = &self.capture_session_id {
            event.insert("capture_session_id".to_owned(), json!(session));
        }
        if let Some(stem) = &self.stem_id {
            event.insert("stem_id".to_owned(), json!(stem));
        }
        event.insert("plugin_instance_id".to_owned(), json!(instance_id));
        event.insert(
            "plugin_capture_session_id".to_owned(),
            json!(plugin_session_id),
        );

        let sequence = event.get("event_sequence").and_then(integer_of);
        let mut receipt_state = "accepted";
        match sequence {
            Some(sequence) if sequence > 0 => {
                let previous = state
                    .streams
                    .state_at(index)
                    .map(|stream| stream.highest_accepted_sequence)
                    .unwrap_or(0);
                if previous > 0 && sequence <= previous {
                    state.counters.sequence_out_of_order =
                        state.counters.sequence_out_of_order.saturating_add(1);
                    let reason = format!(
                        "duplicate/out-of-order sequence for {instance_id}/{plugin_session_id}: \
                         {sequence} <= {previous}"
                    );
                    Self::reject(state, &reason, rejections);
                    if let Some(stream) = state.streams.state_at(index) {
                        stream.rejections = stream.rejections.saturating_add(1);
                    }
                    let known = state.streams.entries.get(index).map(|(_, s)| s.clone());
                    let ack = self.build_ack(
                        state,
                        &event,
                        false,
                        "rejected_duplicate_or_out_of_order",
                        Some(&reason),
                        known,
                    );
                    return PacketOutcome {
                        event: None,
                        event_type: None,
                        acknowledgement: ack,
                    };
                }
                let expected = if previous > 0 { previous + 1 } else { 1 };
                if sequence > expected {
                    let gap = (sequence - expected).max(0) as u64;
                    state.counters.sequence_gaps = state.counters.sequence_gaps.saturating_add(gap);
                    if let Some(stream) = state.streams.state_at(index) {
                        stream.gaps = stream.gaps.saturating_add(gap);
                    }
                    receipt_state = "accepted_with_gap";
                    log::warn!(
                        "UDP sequence gap: instance={instance_id} plugin_session={plugin_session_id} \
                         previous={previous} current={sequence} missing={gap}"
                    );
                }
                if let Some(stream) = state.streams.state_at(index) {
                    stream.highest_accepted_sequence = sequence;
                    if sequence == stream.highest_contiguous_sequence + 1 {
                        stream.highest_contiguous_sequence = sequence;
                    }
                }
            }
            _ => {
                state.counters.events_missing_sequence =
                    state.counters.events_missing_sequence.saturating_add(1);
                receipt_state = "accepted_sequence_unknown";
            }
        }

        if event_type == EventType::BufferHash {
            let expected = state
                .streams
                .state_at(index)
                .and_then(|stream| stream.last_window_hash.clone());
            let previous_hash = event
                .get("prev_hash")
                .map(apw_core::python_str)
                .unwrap_or_default();
            match expected {
                Some(expected) if previous_hash != expected => {
                    state.counters.hash_chain_breaks =
                        state.counters.hash_chain_breaks.saturating_add(1);
                    if let Some(stream) = state.streams.state_at(index) {
                        stream.chain_breaks = stream.chain_breaks.saturating_add(1);
                    }
                    receipt_state = "accepted_chain_break";
                    log::warn!(
                        "Hash-chain break: instance={instance_id} plugin_session={plugin_session_id} \
                         expected={} received={}",
                        prefix12(&expected),
                        prefix12(&previous_hash)
                    );
                }
                None if !previous_hash.is_empty() && previous_hash != "genesis" => {
                    // A first-seen stream cannot vouch for a chain that claims to
                    // continue from windows this receiver never saw.
                    state.counters.hash_chain_breaks =
                        state.counters.hash_chain_breaks.saturating_add(1);
                    if let Some(stream) = state.streams.state_at(index) {
                        stream.chain_breaks = stream.chain_breaks.saturating_add(1);
                    }
                    receipt_state = "accepted_chain_unknown";
                    log::warn!(
                        "Non-genesis chain start: instance={instance_id} \
                         plugin_session={plugin_session_id} prev={}",
                        prefix12(&previous_hash)
                    );
                }
                _ => {}
            }
            let window_hash = event
                .get("window_hash")
                .map(apw_core::python_str)
                .unwrap_or_default();
            if let Some(stream) = state.streams.state_at(index) {
                stream.last_window_hash = Some(window_hash);
            }
        }

        let event_id_prefix = self
            .capture_session_id
            .clone()
            .unwrap_or_else(|| self.receiver_instance_id.clone());
        event.insert(
            "daemon_event_id".to_owned(),
            json!(format!(
                "{event_id_prefix}:{}",
                state.counters.events_received.saturating_add(1)
            )),
        );

        let known = state.streams.entries.get(index).map(|(_, s)| s.clone());
        let ack = self.build_ack(state, &event, true, receipt_state, None, known);
        PacketOutcome {
            event: Some(Value::Object(event)),
            event_type: Some(event_type),
            acknowledgement: ack,
        }
    }

    fn reject(state: &mut ReceiverState, reason: &str, rejections: &mut Vec<String>) {
        state.counters.events_rejected = state.counters.events_rejected.saturating_add(1);
        let count = state.counters.events_rejected;
        if count <= 5 || count % 100 == 0 {
            rejections.push(format!("#{count}: {reason}"));
        }
    }

    fn build_ack(
        &self,
        state: &ReceiverState,
        event: &Map<String, Value>,
        accepted: bool,
        receipt_state: &str,
        reason: Option<&str>,
        stream: Option<StreamReceiptState>,
    ) -> Value {
        let instance_id = fallback_str(event, "plugin_instance_id", "unknown_plugin_instance");
        let plugin_session_id = fallback_str(
            event,
            "plugin_capture_session_id",
            "unknown_plugin_capture_session",
        );
        let stream = stream.or_else(|| {
            state
                .streams
                .peek(&(instance_id.clone(), plugin_session_id.clone()))
                .cloned()
        });
        let mut ack = Map::new();
        ack.insert(
            "message_type".to_owned(),
            json!("daemon_receipt_acknowledgement"),
        );
        ack.insert("protocol".to_owned(), json!(ACK_PROTOCOL));
        ack.insert(
            "daemon_instance_id".to_owned(),
            json!(self.receiver_instance_id),
        );
        ack.insert(
            "daemon_capture_session_id".to_owned(),
            match &self.capture_session_id {
                Some(session) => json!(session),
                None => Value::Null,
            },
        );
        ack.insert("plugin_instance_id".to_owned(), json!(instance_id));
        ack.insert(
            "plugin_capture_session_id".to_owned(),
            json!(plugin_session_id),
        );
        ack.insert(
            "event_sequence".to_owned(),
            event.get("event_sequence").cloned().unwrap_or(Value::Null),
        );
        ack.insert("accepted".to_owned(), json!(accepted));
        ack.insert("receipt_state".to_owned(), json!(receipt_state));
        ack.insert(
            "highest_accepted_sequence".to_owned(),
            json!(stream.as_ref().map(|s| s.highest_accepted_sequence).unwrap_or(0)),
        );
        ack.insert(
            "highest_contiguous_sequence".to_owned(),
            json!(stream
                .as_ref()
                .map(|s| s.highest_contiguous_sequence)
                .unwrap_or(0)),
        );
        ack.insert(
            "stream_gaps".to_owned(),
            json!(stream.as_ref().map(|s| s.gaps).unwrap_or(0)),
        );
        ack.insert(
            "stream_rejections".to_owned(),
            json!(stream.as_ref().map(|s| s.rejections).unwrap_or(0)),
        );
        ack.insert(
            "stream_chain_breaks".to_owned(),
            json!(stream.as_ref().map(|s| s.chain_breaks).unwrap_or(0)),
        );
        ack.insert("daemon_received_at".to_owned(), json!(utc_timestamp(None)));
        ack.insert(
            "operational_scope".to_owned(),
            json!(ACK_OPERATIONAL_SCOPE),
        );
        if let Some(reason) = reason {
            ack.insert("reason".to_owned(), json!(reason));
        }
        Value::Object(ack)
    }

    /// Sends one acknowledgement. The socket write happens with no lock held; the
    /// counters are updated afterwards.
    pub fn send_acknowledgement(&self, address: SocketAddr, acknowledgement: &Value) -> bool {
        let Ok(payload) = canonical_json_utf8(acknowledgement) else {
            self.count_ack(false, false);
            return false;
        };
        let sent = self.socket.send_to(&payload, address);
        match sent {
            Ok(written) if written == payload.len() => {
                self.count_ack(true, true);
                true
            }
            _ => {
                self.count_ack(true, false);
                false
            }
        }
    }

    fn count_ack(&self, attempted: bool, sent: bool) {
        if let Ok(mut state) = self.state.lock() {
            if attempted {
                state.counters.acknowledgements_attempted =
                    state.counters.acknowledgements_attempted.saturating_add(1);
            }
            if sent {
                state.counters.acknowledgements_sent =
                    state.counters.acknowledgements_sent.saturating_add(1);
            } else {
                state.counters.acknowledgements_failed =
                    state.counters.acknowledgements_failed.saturating_add(1);
            }
        }
    }

    pub fn diagnostics(&self) -> ReceiverDiagnostics {
        self.state
            .lock()
            .map(|state| {
                let mut diagnostics = state.counters.clone();
                diagnostics.stream_evictions = state.streams.evictions;
                diagnostics
            })
            .unwrap_or_default()
    }

    /// Restarts the counters and the stream table for a new take.
    ///
    /// IMPORTANT: coverage compares plug-in cumulative counters against this
    /// receiver's counters, so a session reset that clears one side and not the
    /// other makes `complete_observed_path` unreachable. The socket, the bound
    /// port and the daemon instance id survive: they identify this receiver, and
    /// evidence already written commits to them.
    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = ReceiverState::default();
        }
    }

    pub fn tracked_stream_count(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.streams.entries.len())
            .unwrap_or(0)
    }

    /// The signed `daemon_receipt_acknowledgement` record.
    pub fn receipt_summary(&self) -> Value {
        let Ok(state) = self.state.lock() else {
            return json!({
                "protocol": ACK_PROTOCOL,
                "status": "unknown",
                "daemon_instance_id": self.receiver_instance_id,
                "daemon_capture_session_id": self.capture_session_id,
                "streams": [],
                "counters": {"attempted": 0, "sent": 0, "failed": 0},
                "stream_evictions": 0,
                "scope": RECEIPT_SCOPE,
                apw_core::PROOF_LEVEL_KEY: apw_core::ProofLevel::UnknownUnobserved.as_str(),
            });
        };
        let mut snapshot: Vec<(StreamKey, StreamReceiptState)> = state.streams.entries.clone();
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        let streams: Vec<Value> = snapshot
            .iter()
            .map(|((instance_id, plugin_session_id), stream)| {
                json!({
                    "plugin_instance_id": instance_id,
                    "plugin_capture_session_id": plugin_session_id,
                    "highest_accepted_sequence": stream.highest_accepted_sequence,
                    "highest_contiguous_sequence": stream.highest_contiguous_sequence,
                    "gaps": stream.gaps,
                    "rejections": stream.rejections,
                    "chain_breaks": stream.chain_breaks,
                })
            })
            .collect();
        let counters = &state.counters;
        let degraded = counters.acknowledgements_failed > 0
            || counters.acknowledgements_sent < counters.packets_received
            // An evicted stream might have been the subject; once any stream was
            // dropped the receiver cannot vouch for a continuous view of it.
            || state.streams.evictions > 0
            || snapshot
                .iter()
                .any(|(_, stream)| stream.gaps > 0 || stream.rejections > 0 || stream.chain_breaks > 0);
        let status = if snapshot.is_empty() {
            "unknown"
        } else if degraded {
            "degraded"
        } else {
            "issued"
        };
        json!({
            "protocol": ACK_PROTOCOL,
            "status": status,
            "daemon_instance_id": self.receiver_instance_id,
            "daemon_capture_session_id": self.capture_session_id,
            "streams": streams,
            "counters": {
                "attempted": counters.acknowledgements_attempted,
                "sent": counters.acknowledgements_sent,
                "failed": counters.acknowledgements_failed,
            },
            "stream_evictions": state.streams.evictions,
            "scope": RECEIPT_SCOPE,
            // A degraded receipt (dropped stream, ACK gap, chain break) cannot be
            // claimed as a firsthand observation of a complete stream.
            apw_core::PROOF_LEVEL_KEY: if status == "issued" {
                apw_core::ProofLevel::DirectlyObserved.as_str()
            } else {
                apw_core::ProofLevel::UnknownUnobserved.as_str()
            },
        })
    }
}

fn fallback_str(event: &Map<String, Value>, key: &str, fallback: &str) -> String {
    match event.get(key) {
        Some(value) if apw_core::is_truthy(value) => apw_core::python_str(value),
        _ => fallback.to_owned(),
    }
}

/// Python's `isinstance(sequence, int)`: a JSON float is not a sequence number.
fn integer_of(value: &Value) -> Option<i64> {
    value.as_i64()
}

fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn prefix12(value: &str) -> String {
    value.chars().take(12).collect()
}
