//! The three signed documents: a trust anchor, a signer record, and a revocation list.
//!
//! Every one is Ed25519 over `type || 0x00 || apw-json-sort-v1(payload)`. The type string is both
//! the document's declared `type` and its signing domain, so a signature taken over one document
//! cannot be replayed as another, and none of them collides with a manifest signature, which is
//! taken over bare canonical JSON with no domain prefix.

use serde_json::{Map, Value};

use audio_provenance_core::signing::signer_id_for_public_key;
use audio_provenance_core::{SigningKey, canonical_json, verify_domain_separated};

use crate::error::TrustError;
use crate::reader::Reader;
use crate::time::{Instant, Window};

pub const ANCHOR_TYPE: &str = "audio-provenance-trust-anchor-v1";
pub const RECORD_TYPE: &str = "audio-provenance-trust-signer-record-v1";
pub const REVOCATIONS_TYPE: &str = "audio-provenance-trust-revocations-v1";

/// The only signature algorithm this build will chain through. A document naming anything else is
/// refused at parse rather than at use: algorithm agility with no pinned set is how a chain gets
/// downgraded.
pub const ALGORITHM: &str = "ed25519";

/// The absolute ceiling on links between a leaf record and its anchor, independent of what any
/// anchor's own policy asks for. A store is untrusted input; without a constant bound, a deep
/// issuer graph would make every lookup linear in the store's size.
pub const CHAIN_DEPTH_CEILING: u64 = 8;

pub const MAX_ID_LEN: usize = 64;
pub const MAX_NAME_LEN: usize = 128;
pub const MAX_REVOCATION_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// May be resolved to an identity. May not issue.
    Leaf,
    /// May issue further records. Is never itself resolved to an identity.
    Issuer,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Leaf => "leaf",
            Self::Issuer => "issuer",
        }
    }

    pub fn parse(text: &str) -> Result<Self, TrustError> {
        match text {
            "leaf" => Ok(Self::Leaf),
            "issuer" => Ok(Self::Issuer),
            other => Err(TrustError::Malformed {
                what: "signer record",
                reason: format!(
                    "capability must be \"leaf\" or \"issuer\", found {:?}",
                    clip(other)
                ),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    KeyCompromise,
    Superseded,
    CeasedOperation,
    Unspecified,
}

impl RevocationReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KeyCompromise => "key_compromise",
            Self::Superseded => "superseded",
            Self::CeasedOperation => "ceased_operation",
            Self::Unspecified => "unspecified",
        }
    }

    pub fn parse(text: &str) -> Result<Self, TrustError> {
        match text {
            "key_compromise" => Ok(Self::KeyCompromise),
            "superseded" => Ok(Self::Superseded),
            "ceased_operation" => Ok(Self::CeasedOperation),
            "unspecified" => Ok(Self::Unspecified),
            other => Err(TrustError::Malformed {
                what: "revocation entry",
                reason: format!(
                    "reason is not key_compromise, superseded, ceased_operation or unspecified: {:?}",
                    clip(other)
                ),
            }),
        }
    }
}

/// A self-signed authority: a name, a key, a window, and the depth its policy permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub anchor_id: String,
    pub name: String,
    pub public_key: [u8; 32],
    pub window: Window,
    pub max_chain_depth: u64,
}

impl Anchor {
    const SIGNED_FIELDS: [&'static str; 8] = [
        "type",
        "anchor_id",
        "name",
        "algorithm",
        "public_key_hex",
        "not_before",
        "not_after",
        "max_chain_depth",
    ];
    const SIGNATURE_FIELD: &'static str = "self_signature_hex";

    fn payload(&self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("type".to_string(), ANCHOR_TYPE.into());
        payload.insert("anchor_id".to_string(), self.anchor_id.clone().into());
        payload.insert("name".to_string(), self.name.clone().into());
        payload.insert("algorithm".to_string(), ALGORITHM.into());
        payload.insert(
            "public_key_hex".to_string(),
            hex::encode(self.public_key).into(),
        );
        payload.insert(
            "not_before".to_string(),
            self.window.not_before.as_str().into(),
        );
        payload.insert(
            "not_after".to_string(),
            self.window.not_after.as_str().into(),
        );
        payload.insert("max_chain_depth".to_string(), self.max_chain_depth.into());
        payload
    }

    /// The 16-hex key identifier a consumer reads to judge the anchor, shared with
    /// `audio-provenance keygen`'s `signer_id`.
    pub fn key_id(&self) -> String {
        signer_id_for_public_key(&self.public_key)
    }

    /// Self-signs. The key MUST be the anchor's own: a self-signature by any other key is a
    /// document no verifier could ever accept, so it is refused here rather than written to disk.
    pub fn self_sign(&self, key: &SigningKey) -> Result<SignedAnchor, TrustError> {
        require_same_key("anchor self-signature", &self.public_key, key)?;
        let signature = key.sign_domain_separated(ANCHOR_TYPE, &signing_bytes(self.payload())?);
        Ok(SignedAnchor {
            anchor: self.clone(),
            signature,
        })
    }
}

/// An anchor whose self-signature verified. There is no other constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedAnchor {
    anchor: Anchor,
    signature: [u8; 64],
}

impl SignedAnchor {
    pub const fn anchor(&self) -> &Anchor {
        &self.anchor
    }

    pub fn to_value(&self) -> Value {
        with_signature(
            self.anchor.payload(),
            Anchor::SIGNATURE_FIELD,
            &self.signature,
        )
    }

    pub fn parse(value: &Value) -> Result<Self, TrustError> {
        let reader = Reader::new(
            value,
            "trust anchor",
            &fields_with(&Anchor::SIGNED_FIELDS, Anchor::SIGNATURE_FIELD),
        )?;
        reader.expect_type(ANCHOR_TYPE)?;
        expect_algorithm(&reader)?;
        let anchor = Anchor {
            anchor_id: reader.identifier("anchor_id", MAX_ID_LEN)?,
            name: reader.text("name", MAX_NAME_LEN)?,
            public_key: reader.hex::<32>("public_key_hex")?,
            window: Window::new(reader.instant("not_before")?, reader.instant("not_after")?)?,
            max_chain_depth: reader.integer("max_chain_depth", 1..=CHAIN_DEPTH_CEILING)?,
        };
        let signature = reader.hex::<64>(Anchor::SIGNATURE_FIELD)?;
        verify_domain_separated(
            &anchor.public_key,
            ANCHOR_TYPE,
            &signing_bytes(anchor.payload())?,
            &signature,
        )
        .map_err(|_| TrustError::DocumentSignatureInvalid {
            what: "anchor self",
            id: anchor.anchor_id.clone(),
        })?;
        Ok(Self { anchor, signature })
    }
}

/// A binding from a public key to a display name, vouched for by an anchor or by an issuer record
/// that chains to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerRecord {
    pub record_id: String,
    pub subject_public_key: [u8; 32],
    pub display_name: String,
    pub capability: Capability,
    pub issuer_anchor_id: String,
    pub issuer_public_key: [u8; 32],
    pub window: Window,
}

impl SignerRecord {
    const SIGNED_FIELDS: [&'static str; 11] = [
        "type",
        "record_id",
        "subject_public_key_hex",
        "subject_signer_id",
        "display_name",
        "capability",
        "algorithm",
        "issuer_anchor_id",
        "issuer_public_key_hex",
        "not_before",
        "not_after",
    ];
    const SIGNATURE_FIELD: &'static str = "signature_hex";

    fn payload(&self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("type".to_string(), RECORD_TYPE.into());
        payload.insert("record_id".to_string(), self.record_id.clone().into());
        payload.insert(
            "subject_public_key_hex".to_string(),
            hex::encode(self.subject_public_key).into(),
        );
        payload.insert(
            "subject_signer_id".to_string(),
            self.subject_signer_id().into(),
        );
        payload.insert("display_name".to_string(), self.display_name.clone().into());
        payload.insert("capability".to_string(), self.capability.as_str().into());
        payload.insert("algorithm".to_string(), ALGORITHM.into());
        payload.insert(
            "issuer_anchor_id".to_string(),
            self.issuer_anchor_id.clone().into(),
        );
        payload.insert(
            "issuer_public_key_hex".to_string(),
            hex::encode(self.issuer_public_key).into(),
        );
        payload.insert(
            "not_before".to_string(),
            self.window.not_before.as_str().into(),
        );
        payload.insert(
            "not_after".to_string(),
            self.window.not_after.as_str().into(),
        );
        payload
    }

    pub fn subject_signer_id(&self) -> String {
        signer_id_for_public_key(&self.subject_public_key)
    }

    pub fn sign(&self, key: &SigningKey) -> Result<SignedRecord, TrustError> {
        require_same_key("signer record", &self.issuer_public_key, key)?;
        let signature = key.sign_domain_separated(RECORD_TYPE, &signing_bytes(self.payload())?);
        Ok(SignedRecord {
            record: self.clone(),
            signature,
        })
    }
}

/// A parsed record. Its signature is NOT checked here: the issuer key is not known until the chain
/// walk locates it, and checking against a key the document itself supplies would verify nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRecord {
    record: SignerRecord,
    signature: [u8; 64],
}

impl SignedRecord {
    pub const fn record(&self) -> &SignerRecord {
        &self.record
    }

    pub fn to_value(&self) -> Value {
        with_signature(
            self.record.payload(),
            SignerRecord::SIGNATURE_FIELD,
            &self.signature,
        )
    }

    pub fn parse(value: &Value) -> Result<Self, TrustError> {
        let reader = Reader::new(
            value,
            "signer record",
            &fields_with(&SignerRecord::SIGNED_FIELDS, SignerRecord::SIGNATURE_FIELD),
        )?;
        reader.expect_type(RECORD_TYPE)?;
        expect_algorithm(&reader)?;
        let record = SignerRecord {
            record_id: reader.identifier("record_id", MAX_ID_LEN)?,
            subject_public_key: reader.hex::<32>("subject_public_key_hex")?,
            display_name: reader.text("display_name", MAX_NAME_LEN)?,
            capability: Capability::parse(&reader.text("capability", 16)?)?,
            issuer_anchor_id: reader.identifier("issuer_anchor_id", MAX_ID_LEN)?,
            issuer_public_key: reader.hex::<32>("issuer_public_key_hex")?,
            window: Window::new(reader.instant("not_before")?, reader.instant("not_after")?)?,
        };
        let declared = reader.identifier("subject_signer_id", MAX_ID_LEN)?;
        let derived = record.subject_signer_id();
        if declared != derived {
            return Err(TrustError::Inconsistent {
                what: "signer record subject_signer_id",
                declared,
                derived,
            });
        }
        // A record that issues itself is a one-node cycle. Refusing it at parse keeps the chain
        // walk's cycle check about genuine multi-node loops.
        if record.subject_public_key == record.issuer_public_key {
            return Err(TrustError::Inconsistent {
                what: "signer record issuer",
                declared: hex::encode(record.issuer_public_key),
                derived: "a record may not issue itself".to_string(),
            });
        }
        Ok(Self {
            record,
            signature: reader.hex::<64>(SignerRecord::SIGNATURE_FIELD)?,
        })
    }

    /// The chain's only signature check on a record, run once the issuer key is known.
    pub fn verify_under(&self, issuer_public_key: &[u8; 32]) -> Result<(), TrustError> {
        verify_domain_separated(
            issuer_public_key,
            RECORD_TYPE,
            &signing_bytes(self.record.payload())?,
            &self.signature,
        )
        .map_err(|_| TrustError::DocumentSignatureInvalid {
            what: "signer record",
            id: self.record.record_id.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationEntry {
    pub subject_public_key: [u8; 32],
    pub revoked_at: Instant,
    pub reason: RevocationReason,
}

impl RevocationEntry {
    const SIGNED_FIELDS: [&'static str; 4] = [
        "subject_public_key_hex",
        "subject_signer_id",
        "revoked_at",
        "reason",
    ];

    fn payload(&self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert(
            "subject_public_key_hex".to_string(),
            hex::encode(self.subject_public_key).into(),
        );
        payload.insert(
            "subject_signer_id".to_string(),
            signer_id_for_public_key(&self.subject_public_key).into(),
        );
        payload.insert("revoked_at".to_string(), self.revoked_at.as_str().into());
        payload.insert("reason".to_string(), self.reason.as_str().into());
        payload
    }

    fn parse(value: &Value) -> Result<Self, TrustError> {
        let reader = Reader::new(value, "revocation entry", &Self::SIGNED_FIELDS)?;
        let entry = Self {
            subject_public_key: reader.hex::<32>("subject_public_key_hex")?,
            revoked_at: reader.instant("revoked_at")?,
            reason: RevocationReason::parse(&reader.text("reason", 32)?)?,
        };
        let declared = reader.identifier("subject_signer_id", MAX_ID_LEN)?;
        let derived = signer_id_for_public_key(&entry.subject_public_key);
        if declared != derived {
            return Err(TrustError::Inconsistent {
                what: "revocation entry subject_signer_id",
                declared,
                derived,
            });
        }
        Ok(entry)
    }
}

/// Revocations published by one anchor. Only an anchor revokes: an issuer record able to revoke on
/// the anchor's behalf would be a second, weaker key that could erase an identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationList {
    pub anchor_id: String,
    pub issuer_public_key: [u8; 32],
    pub issued_at: Instant,
    pub entries: Vec<RevocationEntry>,
}

impl RevocationList {
    const SIGNED_FIELDS: [&'static str; 6] = [
        "type",
        "anchor_id",
        "algorithm",
        "issuer_public_key_hex",
        "issued_at",
        "entries",
    ];
    const SIGNATURE_FIELD: &'static str = "signature_hex";

    fn payload(&self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("type".to_string(), REVOCATIONS_TYPE.into());
        payload.insert("anchor_id".to_string(), self.anchor_id.clone().into());
        payload.insert("algorithm".to_string(), ALGORITHM.into());
        payload.insert(
            "issuer_public_key_hex".to_string(),
            hex::encode(self.issuer_public_key).into(),
        );
        payload.insert("issued_at".to_string(), self.issued_at.as_str().into());
        payload.insert(
            "entries".to_string(),
            Value::Array(
                self.entries
                    .iter()
                    .map(|entry| Value::Object(entry.payload()))
                    .collect(),
            ),
        );
        payload
    }

    pub fn sign(&self, key: &SigningKey) -> Result<SignedRevocationList, TrustError> {
        require_same_key("revocation list", &self.issuer_public_key, key)?;
        let signature =
            key.sign_domain_separated(REVOCATIONS_TYPE, &signing_bytes(self.payload())?);
        Ok(SignedRevocationList {
            list: self.clone(),
            signature,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRevocationList {
    list: RevocationList,
    signature: [u8; 64],
}

impl SignedRevocationList {
    pub const fn list(&self) -> &RevocationList {
        &self.list
    }

    pub fn to_value(&self) -> Value {
        with_signature(
            self.list.payload(),
            RevocationList::SIGNATURE_FIELD,
            &self.signature,
        )
    }

    pub fn parse(value: &Value) -> Result<Self, TrustError> {
        let reader = Reader::new(
            value,
            "revocation list",
            &fields_with(
                &RevocationList::SIGNED_FIELDS,
                RevocationList::SIGNATURE_FIELD,
            ),
        )?;
        reader.expect_type(REVOCATIONS_TYPE)?;
        expect_algorithm(&reader)?;
        let entries = reader
            .array("entries", MAX_REVOCATION_ENTRIES)?
            .iter()
            .map(RevocationEntry::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let list = RevocationList {
            anchor_id: reader.identifier("anchor_id", MAX_ID_LEN)?,
            issuer_public_key: reader.hex::<32>("issuer_public_key_hex")?,
            issued_at: reader.instant("issued_at")?,
            entries,
        };
        let signature = reader.hex::<64>(RevocationList::SIGNATURE_FIELD)?;
        verify_domain_separated(
            &list.issuer_public_key,
            REVOCATIONS_TYPE,
            &signing_bytes(list.payload())?,
            &signature,
        )
        .map_err(|_| TrustError::DocumentSignatureInvalid {
            what: "revocation list",
            id: list.anchor_id.clone(),
        })?;
        Ok(Self { list, signature })
    }
}

fn signing_bytes(payload: Map<String, Value>) -> Result<Vec<u8>, TrustError> {
    Ok(canonical_json(&Value::Object(payload))?)
}

fn require_same_key(
    what: &'static str,
    declared: &[u8; 32],
    key: &SigningKey,
) -> Result<(), TrustError> {
    let derived = key.public_key_bytes();
    if derived != *declared {
        return Err(TrustError::Inconsistent {
            what,
            declared: hex::encode(declared),
            derived: hex::encode(derived),
        });
    }
    Ok(())
}

fn expect_algorithm(reader: &Reader<'_>) -> Result<(), TrustError> {
    let found = reader.text("algorithm", 32)?;
    if found != ALGORITHM {
        return Err(TrustError::UnexpectedType {
            expected: ALGORITHM,
            found,
        });
    }
    Ok(())
}

fn fields_with(base: &[&'static str], extra: &'static str) -> Vec<&'static str> {
    let mut fields = base.to_vec();
    fields.push(extra);
    fields
}

/// The written document is the signed payload plus one signature key, so there is exactly one list
/// of signed fields and no path by which a field reaches a file outside it.
fn with_signature(mut payload: Map<String, Value>, field: &str, signature: &[u8; 64]) -> Value {
    payload.insert(field.to_string(), hex::encode(signature).into());
    Value::Object(payload)
}

fn clip(text: &str) -> String {
    text.chars().take(32).collect()
}
