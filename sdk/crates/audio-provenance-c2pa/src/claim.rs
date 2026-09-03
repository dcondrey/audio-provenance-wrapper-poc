//! The C2PA claim, and the assertion-store integrity check that the claim makes possible.
//!
//! # What this proves, and what it does not
//!
//! A claim commits to every assertion by hashed URI. Recomputing those hashes proves the assertion
//! store presented alongside the claim is the store the claim was written over: an assertion
//! swapped, reordered inside its own box, or edited in place is caught here.
//!
//! Recomputing the declared URIs is only half of it, and the half that is easy to mistake for the
//! whole. It answers "is everything the claim named still what it named", not "is everything here
//! named". [`AssertionIntegrity`] asks both, because an assertion box the claim never declared is
//! unsigned content sitting inside a signed store, and a caller listing store labels would present
//! it as though it were covered. C2PA calls that `assertion.undeclared`.
//!
//! None of it proves WHO wrote the claim: the claim's own COSE_Sign1 signature is not verified
//! anywhere in this crate. Both checks are needed for a validation state; only one lives here.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use audio_provenance_core::sha256;
use audio_provenance_manifest::C2paStore;
use ciborium::value::Value;

use crate::error::ClaimError;
use crate::jumbf::{LabelledBox, collect};

pub const STORE_LABEL: &str = "c2pa";
pub const CLAIM_V2_LABEL: &str = "c2pa.claim.v2";
pub const CLAIM_V1_LABEL: &str = "c2pa.claim";
pub const ASSERTION_STORE_LABEL: &str = "c2pa.assertions";
pub const SIGNATURE_LABEL: &str = "c2pa.signature";

/// The active manifest superbox.
///
/// REQUIRED: C2PA 2.4, "Locating the Active Manifest", says the active manifest is the LAST C2PA
/// Manifest superbox in the store superbox, not the first. A store carrying an ingredient's
/// manifest alongside the current one holds several, in order, and taking the first reads the
/// asset's history as its present state.
///
/// Everything else in this module works inside the returned payload, which also scopes assertion
/// resolution: a relative URI such as `self#jumbf=c2pa.assertions/c2pa.actions.v2` addresses the
/// assertion store of ITS OWN manifest, and every manifest in a multi-manifest store has one.
///
/// REQUIRED: `store` is the manifest STORE superbox, the one labelled `c2pa`, exactly as
/// `C2paStore` hands it over. Passing a single manifest superbox instead returns
/// [`ClaimError::NoActiveManifest`] rather than parsing whatever box happens to be at depth 1.
pub fn active_manifest(store: &[u8]) -> Result<LabelledBox<'_>, ClaimError> {
    collect(store)?
        .into_iter()
        .rfind(|b| b.parent == Some(STORE_LABEL) && b.depth == 1)
        .ok_or(ClaimError::NoActiveManifest)
}

/// Which claim map the store carries. The version is carried by the JUMBF label, not by a field
/// inside the claim: a v2 claim's CBOR has no `claim_version` key at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimVersion {
    /// `c2pa.claim`, a single `assertions` array.
    ///
    /// TODO: no v1 fixture exists on this machine and c2pa-rs 0.90 writes only v2, so this arm is
    /// derived from the CDDL and is NOT verified against a real tool's output.
    V1,
    /// `c2pa.claim.v2`, splitting `created_assertions` from `gathered_assertions`.
    V2,
}

impl ClaimVersion {
    pub const fn label(self) -> &'static str {
        match self {
            Self::V1 => CLAIM_V1_LABEL,
            Self::V2 => CLAIM_V2_LABEL,
        }
    }

    pub const fn number(self) -> u8 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

/// Whether the claim generator created an assertion itself or gathered it from elsewhere. A v1
/// claim draws no such distinction and reports every assertion as [`Self::Created`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssertionKind {
    Created,
    Gathered,
}

/// A `$hashed-uri-map`: a JUMBF URI and the digest of what it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedUri {
    url: String,
    alg: Option<String>,
    hash: Vec<u8>,
    kind: AssertionKind,
}

impl HashedUri {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The per-URI algorithm override. Absent means the claim's `alg` applies.
    pub fn alg(&self) -> Option<&str> {
        self.alg.as_deref()
    }

    pub fn hash(&self) -> &[u8] {
        &self.hash
    }

    pub fn hash_hex(&self) -> String {
        hex::encode(&self.hash)
    }

    pub const fn kind(&self) -> AssertionKind {
        self.kind
    }

    /// The assertion label the URI addresses, for both the relative form
    /// `self#jumbf=c2pa.assertions/<label>` and the absolute
    /// `self#jumbf=/c2pa/<manifest>/c2pa.assertions/<label>`.
    pub fn assertion_label(&self) -> Option<&str> {
        let prefix = concat!("c2pa.assertions", "/");
        let start = self.url.rfind(prefix)?.checked_add(prefix.len())?;
        self.url.get(start..).filter(|tail| !tail.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct C2paClaim {
    version: ClaimVersion,
    manifest_label: String,
    alg: String,
    title: Option<String>,
    instance_id: Option<String>,
    signature_ref: Option<String>,
    generator_name: Option<String>,
    generator_version: Option<String>,
    assertions: Vec<HashedUri>,
}

impl C2paClaim {
    /// Reads the claim out of a manifest store located by `audio-provenance-manifest`.
    pub fn from_store(store: &C2paStore) -> Result<Self, ClaimError> {
        Self::from_store_bytes(store.bytes())
    }

    pub fn from_store_bytes(store: &[u8]) -> Result<Self, ClaimError> {
        let manifest = active_manifest(store)?;
        let manifest_label = manifest.label.to_string();
        let boxes = collect(manifest.payload)?;
        let (version, claim_box) = boxes
            .iter()
            .find_map(|b| match b.label {
                CLAIM_V2_LABEL => Some((ClaimVersion::V2, b)),
                CLAIM_V1_LABEL => Some((ClaimVersion::V1, b)),
                _ => None,
            })
            .ok_or(ClaimError::NoClaim)?;
        let cbor = claim_box.cbor().ok_or(ClaimError::ClaimNotCbor)?;
        let value: Value =
            ciborium::from_reader(cbor).map_err(|error| ClaimError::MalformedClaim {
                reason: alloc::format!("{error:?}"),
            })?;
        let map = value.as_map().ok_or(ClaimError::MalformedClaim {
            reason: String::from("claim is not a CBOR map"),
        })?;

        let alg = text(map, "alg")
            .ok_or(ClaimError::ClaimField { field: "alg" })?
            .to_string();

        let mut assertions = Vec::new();
        match version {
            ClaimVersion::V2 => {
                push_hashed_uris(
                    map,
                    "created_assertions",
                    AssertionKind::Created,
                    true,
                    &mut assertions,
                )?;
                push_hashed_uris(
                    map,
                    "gathered_assertions",
                    AssertionKind::Gathered,
                    false,
                    &mut assertions,
                )?;
            }
            ClaimVersion::V1 => {
                push_hashed_uris(
                    map,
                    "assertions",
                    AssertionKind::Created,
                    true,
                    &mut assertions,
                )?;
            }
        }

        let generator = entry(map, "claim_generator_info").and_then(Value::as_map);
        Ok(Self {
            version,
            manifest_label,
            alg,
            title: text(map, "dc:title").map(ToString::to_string),
            instance_id: text(map, "instanceID").map(ToString::to_string),
            signature_ref: text(map, "signature").map(ToString::to_string),
            generator_name: generator
                .and_then(|g| text(g, "name"))
                .map(ToString::to_string),
            generator_version: generator
                .and_then(|g| text(g, "version"))
                .map(ToString::to_string),
            assertions,
        })
    }

    pub const fn version(&self) -> ClaimVersion {
        self.version
    }

    /// The `urn:c2pa:...` label of the manifest superbox the claim sits in.
    pub fn manifest_label(&self) -> &str {
        &self.manifest_label
    }

    pub fn alg(&self) -> &str {
        &self.alg
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn instance_id(&self) -> Option<&str> {
        self.instance_id.as_deref()
    }

    pub fn signature_ref(&self) -> Option<&str> {
        self.signature_ref.as_deref()
    }

    pub fn generator_name(&self) -> Option<&str> {
        self.generator_name.as_deref()
    }

    pub fn generator_version(&self) -> Option<&str> {
        self.generator_version.as_deref()
    }

    pub fn assertions(&self) -> &[HashedUri] {
        &self.assertions
    }

    /// Recomputes every hashed URI against the assertion store the claim shipped with.
    pub fn verify_assertions(&self, store: &C2paStore) -> Result<AssertionIntegrity, ClaimError> {
        self.verify_assertions_in(store.bytes())
    }

    pub fn verify_assertions_in(&self, store: &[u8]) -> Result<AssertionIntegrity, ClaimError> {
        let manifest = active_manifest(store)?;
        let boxes = collect(manifest.payload)?;
        let checks: Vec<HashedUriCheck> = self
            .assertions
            .iter()
            .map(|uri| self.check_one(uri, &boxes))
            .collect();

        // The converse question, which recomputing the declared URIs cannot answer: C2PA's
        // `assertion.undeclared`. An assertion box the claim never committed to is unsigned content
        // sitting in a signed store, and a reader listing store labels would present it as though
        // it were covered.
        let declared: Vec<&str> = self
            .assertions
            .iter()
            .filter_map(HashedUri::assertion_label)
            .collect();
        let undeclared = boxes
            .iter()
            .filter(|b| b.parent == Some(ASSERTION_STORE_LABEL))
            .filter(|b| !declared.contains(&b.label))
            .map(|b| b.label.to_string())
            .collect();

        Ok(AssertionIntegrity { checks, undeclared })
    }

    fn check_one(&self, uri: &HashedUri, boxes: &[LabelledBox<'_>]) -> HashedUriCheck {
        let outcome = match uri.alg().unwrap_or(&self.alg) {
            "sha256" => match self.locate(uri, boxes) {
                None => HashedUriOutcome::AssertionMissing,
                Some(payload) => {
                    let computed = sha256(payload);
                    if computed.as_slice() == uri.hash() {
                        HashedUriOutcome::Match
                    } else {
                        HashedUriOutcome::Mismatch {
                            computed: hex::encode(computed),
                        }
                    }
                }
            },
            other => HashedUriOutcome::UnsupportedAlgorithm {
                alg: other.to_string(),
            },
        };
        HashedUriCheck {
            url: uri.url().to_string(),
            kind: uri.kind(),
            outcome,
        }
    }

    fn locate<'a>(&self, uri: &HashedUri, boxes: &[LabelledBox<'a>]) -> Option<&'a [u8]> {
        let label = uri.assertion_label()?;
        boxes
            .iter()
            .find(|b| b.label == label && b.parent == Some(ASSERTION_STORE_LABEL))
            .map(|b| b.payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashedUriOutcome {
    Match,
    Mismatch {
        computed: String,
    },
    /// The claim commits to an assertion the store does not carry. C2PA calls this
    /// `assertion.missing`; it is a store-integrity failure, never a "no provenance" result.
    AssertionMissing,
    UnsupportedAlgorithm {
        alg: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedUriCheck {
    pub url: String,
    pub kind: AssertionKind,
    pub outcome: HashedUriOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionIntegrity {
    checks: Vec<HashedUriCheck>,
    undeclared: Vec<String>,
}

impl AssertionIntegrity {
    pub fn checks(&self) -> &[HashedUriCheck] {
        &self.checks
    }

    /// Labels present in the active manifest's assertion store that the claim never committed to:
    /// C2PA's `assertion.undeclared`.
    pub fn undeclared(&self) -> &[String] {
        &self.undeclared
    }

    /// True only when every hashed URI the claim commits to recomputed exactly AND the store holds
    /// no assertion the claim did not declare. An unsupported algorithm is NOT intact: it is an
    /// unanswered question, and answering it "yes" would be the same false statement as a forged
    /// match.
    pub fn intact(&self) -> bool {
        !self.checks.is_empty()
            && self.undeclared.is_empty()
            && self
                .checks
                .iter()
                .all(|c| c.outcome == HashedUriOutcome::Match)
    }
}

fn push_hashed_uris(
    map: &[(Value, Value)],
    field: &'static str,
    kind: AssertionKind,
    required: bool,
    out: &mut Vec<HashedUri>,
) -> Result<(), ClaimError> {
    let Some(value) = entry(map, field) else {
        if required {
            return Err(ClaimError::ClaimField { field });
        }
        return Ok(());
    };
    let list = value.as_array().ok_or(ClaimError::ClaimField { field })?;
    for item in list {
        let entries = item.as_map().ok_or(ClaimError::MalformedHashedUri {
            field,
            reason: "hashed URI is not a CBOR map",
        })?;
        let url = text(entries, "url").ok_or(ClaimError::MalformedHashedUri {
            field,
            reason: "hashed URI carries no url",
        })?;
        let hash = entry(entries, "hash").and_then(Value::as_bytes).ok_or(
            ClaimError::MalformedHashedUri {
                field,
                reason: "hashed URI carries no hash byte string",
            },
        )?;
        out.push(HashedUri {
            url: url.to_string(),
            alg: text(entries, "alg").map(ToString::to_string),
            hash: hash.clone(),
            kind,
        });
    }
    Ok(())
}

fn entry<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

fn text<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a str> {
    entry(map, key).and_then(Value::as_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_assertion_label_is_read_from_both_uri_forms() {
        let relative = HashedUri {
            url: String::from("self#jumbf=c2pa.assertions/c2pa.hash.data"),
            alg: None,
            hash: Vec::new(),
            kind: AssertionKind::Created,
        };
        let absolute = HashedUri {
            url: String::from("self#jumbf=/c2pa/urn:c2pa:x/c2pa.assertions/c2pa.actions.v2__1"),
            alg: None,
            hash: Vec::new(),
            kind: AssertionKind::Gathered,
        };
        let elsewhere = HashedUri {
            url: String::from("self#jumbf=/c2pa/urn:c2pa:x/c2pa.signature"),
            alg: None,
            hash: Vec::new(),
            kind: AssertionKind::Created,
        };
        assert_eq!(relative.assertion_label(), Some("c2pa.hash.data"));
        assert_eq!(absolute.assertion_label(), Some("c2pa.actions.v2__1"));
        assert_eq!(elsewhere.assertion_label(), None);
    }

    #[test]
    fn an_empty_check_list_is_never_intact() {
        assert!(
            !AssertionIntegrity {
                checks: Vec::new(),
                undeclared: Vec::new(),
            }
            .intact()
        );
    }
}
