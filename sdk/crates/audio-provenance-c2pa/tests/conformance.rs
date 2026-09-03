//! Conformance against a real C2PA tool, pinned as literals.
//!
//! FIXTURE PROVENANCE, stated exactly, because a negative fixture is worth nothing if it came from
//! the reader it is testing.
//!
//! - `poc_unobserved_export_c2pa.wav`: copied byte for byte from the audio provenance POC's founder
//!   evidence package, `demo-output/founder-package/evidence-package-20260831T091551Z/honest-null/
//!   artifacts/`. Signed by the POC's `daemon/c2pa_engine` through c2pa-rs 0.90.15.
//! - `poc_sample.aiff` / `poc_sample_sidecar.c2pa`: copied from
//!   `crates/audio-provenance-manifest/tests/fixtures/`, where another effort placed them after running the
//!   POC's signer. They are not in `demo-output/`. Real c2pa-rs output, and c2patool validates the
//!   pair, which is why they are used here.
//! - `c2patool_two_manifests.wav`: written by `c2patool` itself, signing an already-signed asset
//!   with `-p` so the first manifest becomes the ingredient's and the second the active one.
//! - `undeclared_assertion_sidecar.c2pa`: a negative fixture, built by splicing a verbatim
//!   assertion superbox from the first fixture into the sidecar's assertion store. See the test
//!   that uses it.
//!
//! Nothing here was produced by the code under test.
//!
//! GROUND TRUTH. Every literal below was read out of `c2patool 0.26.68 -d` output, or out of the
//! signed CBOR by a throwaway Python probe that knows nothing about this crate. c2patool reports
//! `validation_state: Valid` on the two signed fixtures, with `assertion.dataHash.match` and one
//! `assertion.hashedURI.match` per assertion, and `signingCredential.untrusted` as its only
//! failure. `tests/c2patool_differential.rs` re-derives all of it by running the tool.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use audio_provenance_c2pa::{
    AssertionKind, C2paClaim, C2paEvidence, C2paValidationState, ClaimSignatureEvidence,
    ClaimVersion, CredentialEvidence, HardBindingEvidence, HashedUriOutcome, classify,
};
use audio_provenance_core::CodedError;
use audio_provenance_manifest::{BindingOutcome, C2paStore, StoreLocation};

const EMBEDDED_WAV: &[u8] = include_bytes!("fixtures/poc_unobserved_export_c2pa.wav");
const SIDECAR: &[u8] = include_bytes!("fixtures/poc_sample_sidecar.c2pa");
const SIDECAR_ASSET: &[u8] = include_bytes!("fixtures/poc_sample.aiff");
const TWO_MANIFESTS: &[u8] = include_bytes!("fixtures/c2patool_two_manifests.wav");
const UNDECLARED_SIDECAR: &[u8] = include_bytes!("fixtures/undeclared_assertion_sidecar.c2pa");

const WAV_MANIFEST_LABEL: &str = "urn:c2pa:1f81afda-6a07-4448-aded-67b632800127";
const WAV_HASH_DATA_URI: &str = "ad5054ea6a1005b191e936dc79eac456ffc57bc345b53f9c67b79b3a3dd2e8c8";
const WAV_ACTIONS_URI: &str = "4cc2eef1f6a17d48eeba112a2f1e8a09e4c6f019c99d94cd124d17db31de0221";
const WAV_UNOBSERVED_URI: &str = "62138f9cb9b4d367943ae80e924ec2244b634e413f23833d69ed4e480d82eff7";
const WAV_HARD_BINDING: &str = "bd94fbc105b55383077714246ba7dbf6f73282ae214879a054d37d3cc3455c36";

const SIDECAR_MANIFEST_LABEL: &str = "urn:c2pa:b7f007f9-4983-43a3-b699-2368fefbebe5";
const SIDECAR_HASH_DATA_URI: &str =
    "fa73f4fa674e1d45aec7c849b286a972344c6a147e2f3bc02a10785c81e32fb8";
const ACTIVE_OF_TWO: &str = "urn:c2pa:527c8a67-0d38-4af5-ae27-1d78ef5b5eb1";

#[test]
fn the_embedded_claim_matches_what_c2patool_reports_field_for_field() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();

    assert_eq!(claim.version(), ClaimVersion::V2);
    assert_eq!(claim.manifest_label(), WAV_MANIFEST_LABEL);
    assert_eq!(claim.alg(), "sha256");
    assert_eq!(claim.title(), Some("unobserved_export.wav"));
    assert_eq!(
        claim.instance_id(),
        Some("xmp:iid:ddc85b77-36e0-4169-8057-9e74f101885c")
    );
    assert_eq!(claim.generator_name(), Some("audio-provenance-wrapper"));
    assert_eq!(claim.generator_version(), Some("1.0.0"));
    assert_eq!(
        claim.signature_ref(),
        Some("self#jumbf=/c2pa/urn:c2pa:1f81afda-6a07-4448-aded-67b632800127/c2pa.signature")
    );

    let listed: Vec<(&str, AssertionKind, String)> = claim
        .assertions()
        .iter()
        .map(|u| (u.assertion_label().unwrap(), u.kind(), u.hash_hex()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (
                "c2pa.hash.data",
                AssertionKind::Created,
                WAV_HASH_DATA_URI.to_owned()
            ),
            (
                "c2pa.actions.v2",
                AssertionKind::Gathered,
                WAV_ACTIONS_URI.to_owned()
            ),
            (
                "apw.unobserved",
                AssertionKind::Gathered,
                WAV_UNOBSERVED_URI.to_owned()
            ),
        ]
    );
}

/// The check nothing in the workspace could make before: c2patool emits one
/// `assertion.hashedURI.match` per assertion, and this recomputes the same three digests from the
/// store's own bytes. Agreement here is what makes "we read C2PA" a claim about the format rather
/// than about our writer.
#[test]
fn every_hashed_uri_recomputes_over_the_assertion_store_c2patool_accepted() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();
    let integrity = claim.verify_assertions(&store).unwrap();

    assert_eq!(integrity.checks().len(), 3);
    for check in integrity.checks() {
        assert_eq!(check.outcome, HashedUriOutcome::Match, "{}", check.url);
    }
    assert!(integrity.intact());
}

/// One byte flipped inside an assertion's CBOR must break exactly that hashed URI and no other.
/// Locating the byte through the store's own structure keeps this from degenerating into a test of
/// a magic offset.
#[test]
fn editing_one_assertion_breaks_only_its_own_hashed_uri() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();

    let cbor = store.assertion("apw.unobserved").unwrap().unwrap();
    let target = EMBEDDED_WAV
        .windows(cbor.len())
        .position(|w| w == cbor.as_slice())
        .expect("the assertion's CBOR appears verbatim in the container");

    let mut tampered = EMBEDDED_WAV.to_vec();
    tampered[target] ^= 0x01;

    let tampered_store = C2paStore::from_wav(&tampered).unwrap();
    let integrity = claim.verify_assertions(&tampered_store).unwrap();
    assert!(!integrity.intact());

    let broken: Vec<&str> = integrity
        .checks()
        .iter()
        .filter(|c| c.outcome != HashedUriOutcome::Match)
        .map(|c| c.url.as_str())
        .collect();
    assert_eq!(broken, vec!["self#jumbf=c2pa.assertions/apw.unobserved"]);
}

/// The hard binding is `audio-provenance-manifest`'s to recompute; this pins the value it agrees with
/// c2patool on, and the exclusion the POC actually signed.
#[test]
fn the_hard_binding_recomputes_to_the_digest_c2patool_validated() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let binding = store.hard_binding().unwrap();

    assert_eq!(binding.digest_hex(), WAV_HARD_BINDING);
    assert_eq!(binding.name(), Some("jumbf manifest"));
    assert_eq!(binding.exclusions().len(), 1);
    assert_eq!(binding.exclusions()[0].start, 98_348);
    assert_eq!(binding.exclusions()[0].length, 13_625);
    assert_eq!(
        binding.evaluate(EMBEDDED_WAV).unwrap(),
        BindingOutcome::Match
    );

    let mut altered = EMBEDDED_WAV.to_vec();
    altered[50_000] ^= 0x01;
    assert_eq!(
        binding.evaluate(&altered).unwrap(),
        BindingOutcome::Mismatch
    );
}

/// CONFORMANCE FINDING, pinned so it cannot be lost.
///
/// `StoreLocation::EmbeddedWavChunk::length` is documented in `audio-provenance-manifest` as "exactly the
/// region the hard binding excludes". It is not. c2pa-rs excludes the RIFF chunk header plus its
/// declared payload and stops there; the RIFF pad byte that follows an odd-length payload is
/// INSIDE the hash. Every embedded manifest the POC has produced has an odd payload, so the two
/// differ by one byte on all of them. `evaluate` is unaffected because it reads the signed
/// exclusion list, not this field.
#[test]
fn the_reported_chunk_extent_overshoots_the_signed_exclusion_by_the_riff_pad_byte() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let StoreLocation::EmbeddedWavChunk { offset, length } = store.location() else {
        panic!("the fixture carries an embedded store");
    };
    let signed = store.hard_binding().unwrap();
    let excluded = signed.exclusions()[0];

    assert_eq!(offset as u64, excluded.start);
    assert_eq!(length, 13_626);
    assert_eq!(excluded.length, 13_625);
    // The payload length is odd, so RIFF pads to an even boundary and c2pa-rs leaves that pad byte
    // covered by the binding.
    assert_eq!(store.bytes().len() % 2, 1);
    assert_eq!(length as u64 - excluded.length, 1);
    assert_eq!(offset + length, EMBEDDED_WAV.len());
}

#[test]
fn the_aiff_sidecar_claim_and_assertion_store_agree_with_c2patool() {
    let store = C2paStore::from_sidecar(SIDECAR).unwrap();
    assert_eq!(store.location(), StoreLocation::Sidecar);

    let claim = C2paClaim::from_store(&store).unwrap();
    assert_eq!(claim.version(), ClaimVersion::V2);
    assert_eq!(claim.manifest_label(), SIDECAR_MANIFEST_LABEL);
    assert_eq!(claim.title(), Some("sample.aiff"));
    assert_eq!(claim.assertions().len(), 4);
    assert_eq!(claim.assertions()[0].hash_hex(), SIDECAR_HASH_DATA_URI);

    let integrity = claim.verify_assertions(&store).unwrap();
    assert!(integrity.intact(), "{:?}", integrity.checks());

    // A detached store declares no exclusions, so the binding covers every byte of the AIFF beside
    // it and the digest is the file's own SHA-256.
    let binding = store.hard_binding().unwrap();
    assert!(binding.exclusions().is_empty());
    assert_eq!(
        binding.evaluate(SIDECAR_ASSET).unwrap(),
        BindingOutcome::Match
    );
    assert_eq!(
        binding.digest_hex(),
        audio_provenance_core::sha256_hex(SIDECAR_ASSET)
    );
}

/// The mapping, driven by the facts c2patool actually reported about these fixtures: signature
/// valid, assertions intact, data hash matched, credential untrusted.
#[test]
fn c2patools_own_findings_on_these_fixtures_map_to_untrusted() {
    let store = C2paStore::from_wav(EMBEDDED_WAV).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();
    let binding = store.hard_binding().unwrap();

    let as_c2patool_saw_it = C2paEvidence {
        store_present: true,
        assertion_store_intact: claim.verify_assertions(&store).unwrap().intact(),
        signature: ClaimSignatureEvidence::Validated,
        hard_binding: HardBindingEvidence::from_outcome(binding.evaluate(EMBEDDED_WAV).unwrap()),
        credential: CredentialEvidence::Untrusted,
    };
    assert_eq!(
        classify(as_c2patool_saw_it),
        C2paValidationState::MarkFoundClaimNotTrusted
    );

    // What this crate can honestly source by itself is strictly weaker, and lands in the same
    // place for a different reason: nobody verified the signature.
    let as_this_crate_can_see_it = C2paEvidence {
        signature: ClaimSignatureEvidence::NotEvaluated,
        credential: CredentialEvidence::Trusted,
        ..as_c2patool_saw_it
    };
    assert_eq!(
        classify(as_this_crate_can_see_it),
        C2paValidationState::MarkFoundClaimNotTrusted
    );
}

/// CONFORMANCE, and the check the single-manifest fixtures structurally cannot make.
///
/// C2PA 2.4, "Locating the Active Manifest": the active manifest is the LAST C2PA Manifest superbox
/// in the store, not the first. `c2patool_two_manifests.wav` was signed twice by c2patool, so the
/// first manifest is the ingredient's and the second is the current one; c2patool names the second
/// as `active_manifest`. Reading the first would report an asset's history as its present state.
#[test]
fn the_active_manifest_is_the_last_one_in_the_store_not_the_first() {
    let store = C2paStore::from_wav(TWO_MANIFESTS).unwrap();

    let labels = store.labels().unwrap();
    let manifests: Vec<&String> = labels
        .iter()
        .filter(|l| l.starts_with("urn:c2pa:"))
        .collect();
    assert_eq!(
        manifests,
        vec![
            &"urn:c2pa:952ba218-1d79-4c7a-92ae-f6e83bc1da07".to_owned(),
            &"urn:c2pa:527c8a67-0d38-4af5-ae27-1d78ef5b5eb1".to_owned(),
        ],
        "document order"
    );

    let claim = C2paClaim::from_store(&store).unwrap();
    assert_eq!(claim.manifest_label(), ACTIVE_OF_TWO);
    assert_eq!(claim.title(), Some("twice.wav"));

    // The assertion store resolved must be the ACTIVE manifest's, not the ingredient manifest's:
    // both carry a `c2pa.assertions` superbox and a relative URI names neither explicitly.
    let integrity = claim.verify_assertions(&store).unwrap();
    assert!(integrity.intact(), "{:?}", integrity.checks());
}

/// CONFORMANCE. An assertion box the claim never declared is unsigned content in a signed store.
/// c2patool refuses the whole store for it (`Error: assertion missing: url = apw.smuggledxx`,
/// exit 1, no report at all), so `intact()` must be false here or it would be the more permissive
/// of the two readers.
///
/// FIXTURE PROVENANCE: a verbatim `apw.unobserved` assertion superbox lifted out of
/// `poc_unobserved_export_c2pa.wav`, its 14-byte label overwritten with an equal-length one so the
/// box does not collide, spliced into the sidecar's assertion store with the three enclosing box
/// lengths corrected. c2patool supplied the verdict; it does not know this crate exists.
#[test]
fn an_assertion_the_claim_never_declared_is_not_intact() {
    let store = C2paStore::from_sidecar(UNDECLARED_SIDECAR).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();
    let integrity = claim.verify_assertions(&store).unwrap();

    // Every DECLARED assertion still recomputes: this is exactly the case a hashed-URI-only check
    // would wave through.
    for check in integrity.checks() {
        assert_eq!(check.outcome, HashedUriOutcome::Match, "{}", check.url);
    }
    assert_eq!(integrity.undeclared(), ["apw.smuggledxx"]);
    assert!(!integrity.intact());
}

/// A well-formed WAV with no C2PA chunk is absence, not failure: `NotPresent`, and the store has to
/// say so distinctly from a malformed container.
#[test]
fn a_wav_carrying_no_provenance_reports_absence_and_classifies_as_nothing_found() {
    let StoreLocation::EmbeddedWavChunk { offset, .. } =
        C2paStore::from_wav(EMBEDDED_WAV).unwrap().location()
    else {
        panic!("the fixture carries an embedded store");
    };
    let mut stripped = EMBEDDED_WAV[..offset].to_vec();
    let riff_size = u32::try_from(stripped.len() - 8).unwrap();
    stripped[4..8].copy_from_slice(&riff_size.to_le_bytes());

    assert_eq!(
        C2paStore::from_wav(&stripped).unwrap_err().code(),
        "c2pa_store_absent"
    );
    assert_eq!(
        classify(C2paEvidence::nothing_found()),
        C2paValidationState::NothingFound
    );
}
