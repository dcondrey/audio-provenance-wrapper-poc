use super::statistic::DriveReport;
use serde::Serialize;
use std::collections::BTreeMap;

/// What the attacker has to possess for a row to be reachable.
///
/// IMPORTANT: a removal rate without this column is not a finding. An attack that needs the profile
/// key is not an attack, and one that needs nothing but the published specification is a different
/// class of result from one that needs a hundred licensee copies of the same master.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttackerKnowledge {
    /// The published algorithm specification and nothing else.
    SpecOnly,
    /// One marked file, contents unknown.
    OneMarkedFile,
    /// One marked file whose payload the attacker knows. A registered work's locator is registry
    /// data, so this is the ordinary condition for any published, registered release.
    MarkedFileAndKnownPayload,
    /// Several differently marked copies of the SAME recording under one key, which is a licensee,
    /// screener or per-recipient-forensic scenario rather than a released track.
    ManyMarkedCopiesOfOneRecording,
    /// The profile key. Included only as a control; possession of the key is not an attack.
    ProfileKey,
}

impl AttackerKnowledge {
    pub const fn describe(self) -> &'static str {
        match self {
            Self::SpecOnly => "the published specification only",
            Self::OneMarkedFile => "one marked file",
            Self::MarkedFileAndKnownPayload => "one marked file and its published payload",
            Self::ManyMarkedCopiesOfOneRecording => {
                "several differently marked copies of one recording under one key"
            }
            Self::ProfileKey => "the profile key (control, not an attack)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttackFamily {
    Control,
    Estimation,
    Desynchronisation,
    Collusion,
    Overwriting,
    Forgery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalVerdict {
    /// The unattacked marked copy did not decode, so this row measures nothing about the attack.
    BaselineMissing,
    /// The detector still returns the true payload.
    Survived,
    /// The detector returns nothing.
    Erased,
    /// The detector returns a payload that is not the true one.
    Substituted,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttackRow {
    pub item: String,
    pub attack: String,
    pub family: AttackFamily,
    pub knowledge: AttackerKnowledge,
    pub knowledge_note: &'static str,
    pub params: BTreeMap<String, serde_json::Value>,
    pub verdict: RemovalVerdict,
    /// Whether the attack achieved ITS OWN goal, which is not the same question as the verdict. A
    /// removal wants `erased`; a forgery wants the detector to accept the payload it planted, which
    /// reads as `survived` in the verdict column.
    pub attack_succeeded: bool,
    pub expected_payload_hex: String,
    pub baseline_payload_hex: Option<String>,
    pub attacked_payload_hex: Option<String>,
    pub baseline_class: Option<String>,
    pub attacked_class: Option<String>,
    pub baseline_blocks_accepted: usize,
    pub attacked_blocks_accepted: usize,
    /// False when the attack changes the length or the time base, in which case a sample-aligned
    /// perceptual figure against the original is not a defined quantity and is reported as null.
    pub length_preserving: bool,
    pub segmental_snr_db: Option<f64>,
    pub noise_to_mask_mean_db: Option<f64>,
    pub noise_to_mask_max_db: Option<f64>,
    pub peak_residual_dbfs: Option<f64>,
    pub drive: Option<DriveReport>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttackReport {
    pub algorithm: String,
    pub band_hz: (f64, f64),
    pub lattice_step_nepers: f64,
    pub items: Vec<String>,
    pub rows: Vec<AttackRow>,
    pub perceptual_limits: &'static str,
    pub threat_model: &'static str,
}

pub const THREAT_MODEL: &str = "WHAT THIS MEASURES. A MOTIVATED adversary who has read the \
algorithm specification and wants the mark gone, replaced, or forged. That is a different threat \
model from the incidental degradation the channel matrix measures, and a number from one says \
nothing about the other. Every row states what the attacker must possess; rows whose knowledge \
column names material an ordinary recipient does not hold are not claims about a released file. \
Removal is scored against the unattacked marked copy of the SAME item, so an item that never \
carried the mark cannot inflate a removal rate. WHAT IT DOES NOT MEASURE. Nothing here exercises \
the hash-chain hard binding, the signature, or the signed reference constellation, all of which sit \
above the watermark and are what a verification verdict actually rests on. A payload the detector \
accepts is a registry lookup, never a verdict.";

pub fn classify(
    expected: &[u8],
    baseline: Option<&[u8]>,
    attacked: Option<&[u8]>,
) -> RemovalVerdict {
    match baseline {
        Some(payload) if payload == expected => match attacked {
            None => RemovalVerdict::Erased,
            Some(found) if found == expected => RemovalVerdict::Survived,
            Some(_) => RemovalVerdict::Substituted,
        },
        _ => RemovalVerdict::BaselineMissing,
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
