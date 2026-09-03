#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;

use apw_core::{canonical_json_utf8, sha256_file, ProofLevel, VerificationState};
use apw_provenance::{
    validate_certificate_chain, LocalReferenceProvider, MatchedBy, ProvenanceError,
    ProvenanceProvider,
};
use serde_json::json;
use x509_parser::oid_registry::{
    Oid, OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER, OID_X509_EXT_SUBJECT_KEY_IDENTIFIER,
};
use x509_parser::prelude::*;

const SAMPLE_RATE: u32 = 8_000;

/// Issuance, reload, and the exact certificate profile c2pa-rs was measured
/// against. The provider is constructed twice over the same store so the second
/// pass exercises the reload path, where a rebuilt issuer would produce a leaf
/// whose issuer name or authorityKeyIdentifier no longer matches the stored root.
#[test]
fn an_issued_chain_validates_across_a_reload_and_a_self_signed_certificate_does_not() {
    let store = tempfile::tempdir().unwrap();
    let first = LocalReferenceProvider::new(store.path()).unwrap();
    let first_chain = first.chain_pem();
    let first_key_id = first.key_id().to_string();
    drop(first);

    let provider = LocalReferenceProvider::new(store.path()).unwrap();
    assert_eq!(provider.key_id(), first_key_id);
    assert_eq!(provider.chain_pem(), first_chain);

    let anchor = provider.trust_anchor_pem();
    let validation = validate_certificate_chain(&provider.chain_pem(), &anchor);
    assert!(validation.valid, "{}", validation.reason);

    let der: Vec<Vec<u8>> = Pem::iter_from_buffer(&provider.chain_pem())
        .map(|entry| entry.unwrap().contents)
        .collect();
    assert_eq!(der.len(), 2, "chain must be leaf-then-root");
    let (_, leaf) = parse_x509_certificate(&der[0]).unwrap();
    let (_, root) = parse_x509_certificate(&der[1]).unwrap();

    let leaf_basic = leaf.basic_constraints().unwrap().unwrap();
    assert!(!leaf_basic.value.ca, "leaf must assert CA:FALSE");
    assert!(leaf_basic.critical, "leaf basicConstraints must be critical");

    let leaf_usage = leaf.key_usage().unwrap().unwrap();
    assert!(leaf_usage.critical, "leaf keyUsage must be critical");
    assert!(leaf_usage.value.digital_signature());
    assert!(!leaf_usage.value.key_cert_sign());

    let leaf_eku = leaf.extended_key_usage().unwrap().unwrap();
    assert!(leaf_eku.critical, "leaf EKU must be critical");
    assert!(leaf_eku.value.email_protection);

    let root_basic = root.basic_constraints().unwrap().unwrap();
    assert!(root_basic.critical && root_basic.value.ca);
    assert_eq!(root_basic.value.path_len_constraint, Some(1));
    let root_usage = root.key_usage().unwrap().unwrap();
    assert!(root_usage.critical && root_usage.value.key_cert_sign());

    assert_eq!(
        key_identifier(&leaf, &OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER),
        key_identifier(&root, &OID_X509_EXT_SUBJECT_KEY_IDENTIFIER),
        "leaf authorityKeyIdentifier must match the root subjectKeyIdentifier"
    );

    // A single self-signed certificate is not a chain, whatever its extensions say.
    let self_signed = validate_certificate_chain(&anchor, &anchor);
    assert!(!self_signed.valid);
    assert_eq!(self_signed.reason, "the certificate was self-signed");

    // A well-formed chain still fails against an anchor that did not issue it.
    let other_store = tempfile::tempdir().unwrap();
    let other = LocalReferenceProvider::new(other_store.path()).unwrap();
    let foreign = validate_certificate_chain(&provider.chain_pem(), &other.trust_anchor_pem());
    assert!(!foreign.valid);
    assert_eq!(
        foreign.reason,
        "chain does not terminate at a configured trust anchor"
    );
}

/// Revocation blocks new signatures and keeps every prior record. Losing history
/// on revocation would erase what a creator signed, which this interface does not
/// offer.
#[test]
fn revocation_blocks_new_signatures_and_retains_signing_history() {
    let store = tempfile::tempdir().unwrap();
    let provider = LocalReferenceProvider::new(store.path()).unwrap();
    let key_id = provider.key_id().to_string();

    provider.sign_claim(b"first claim").unwrap();
    provider.sign_claim(b"second claim").unwrap();
    let before = provider.signing_history(&key_id).unwrap();
    assert_eq!(before.len(), 2);

    let record = provider.revoke(&key_id).unwrap();
    assert_eq!(record.retained_signing_records, 2);

    match provider.sign_claim(b"third claim") {
        Err(ProvenanceError::RevokedKey(error)) => {
            assert_eq!(error.key_id, key_id);
            assert_eq!(error.attempted, "sign new claims");
        }
        other => panic!("a revoked key must refuse to sign: {other:?}"),
    }
    assert!(matches!(
        provider.issue_signing_material(),
        Err(ProvenanceError::RevokedKey(_))
    ));

    let after = provider.signing_history(&key_id).unwrap();
    assert_eq!(after, before, "revocation must retain every signing record");
    let identity = provider.identity().unwrap();
    assert!(identity.identity.revoked);
    // The published identity is a hand-written wire shape, so its key order is
    // pinned here rather than left to a reviewer's reading.
    let published = serde_json::to_value(&identity).unwrap();
    let keys: Vec<&str> = published
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "key_id",
            "subject_common_name",
            "algorithm",
            "identity_evidence",
            "revoked",
            "created_at_ms",
            "trust_anchor_sha256",
            "apw:proof_level",
            "limits",
        ]
    );
    assert_eq!(published["apw:proof_level"], "user_declared");

    // The retained history survives a restart: it is on-disk evidence, not state.
    let reopened = LocalReferenceProvider::new(store.path()).unwrap();
    assert_eq!(reopened.signing_history(&key_id).unwrap(), before);
}

/// Every one of the four verification states, reached through the real lookup and
/// trust path rather than by constructing the enum.
#[test]
fn verification_reaches_each_of_the_four_states() {
    let store = tempfile::tempdir().unwrap();
    let provider = LocalReferenceProvider::new(store.path()).unwrap();
    let asset = store.path().join("export.wav");
    write_pcm_wav(&asset, 6.5);
    let original = std::fs::read(&asset).unwrap();

    let nothing = provider.verify(&asset, None).unwrap();
    assert_eq!(nothing.state, VerificationState::NothingFound);
    assert_eq!(nothing.matched_by, MatchedBy::None);
    assert_eq!(nothing.proof_level, ProofLevel::UnknownUnobserved);
    assert!(nothing.mark.is_none());
    assert_eq!(nothing.reason, "no mark and no registry record");

    let attachment = provider
        .embed_mark(&asset, &json!({"session": "demo"}))
        .unwrap();
    assert!(!attachment.asset_modified, "no sample may be altered");
    assert_eq!(attachment.proof_level, ProofLevel::Inferred);
    let recovered = provider.recover_mark(&asset).unwrap().unwrap();
    assert_eq!(recovered.mark_id, attachment.mark_id);
    assert_eq!(recovered.match_similarity, 1.0);
    assert_eq!(recovered.proof_level, ProofLevel::Inferred);

    let unbacked = provider.verify(&asset, None).unwrap();
    assert_eq!(unbacked.state, VerificationState::MarkFoundClaimNotTrusted);
    assert_eq!(
        unbacked.reason,
        "provenance data was presented but no registry record backs it"
    );

    let manifest = json!({
        "content_sha256": sha256_file(&asset).unwrap(),
        "mark_id": attachment.mark_id,
    });
    let manifest_bytes = canonical_json_utf8(&manifest).unwrap();
    let receipt = provider.register(&manifest).unwrap();

    let verified = provider.verify(&asset, Some(&manifest_bytes)).unwrap();
    assert_eq!(verified.state, VerificationState::Verified);
    assert_eq!(verified.matched_by, MatchedBy::ContentSha256);
    assert_eq!(verified.proof_level, ProofLevel::DirectlyObserved);
    assert_eq!(verified.registry_id.as_deref(), Some(receipt.registry_id.as_str()));

    let mut altered = original.clone();
    altered[10_000] ^= 0x40;
    std::fs::write(&asset, &altered).unwrap();
    let changed = provider.verify(&asset, Some(&manifest_bytes)).unwrap();
    assert_eq!(changed.state, VerificationState::RegisteredButChanged);
    assert_eq!(changed.matched_by, MatchedBy::ManifestSha256);
    assert_eq!(
        changed.reason,
        "asset bytes differ from the registered content digest"
    );

    // With no manifest to match, the lookup falls through to the soft binding, and
    // the reported proof level is demoted to `inferred` because a mark is
    // resemblance rather than identity.
    let by_mark = provider.verify(&asset, None).unwrap();
    assert_eq!(by_mark.matched_by, MatchedBy::MarkId);
    assert_eq!(by_mark.proof_level, ProofLevel::Inferred);
    assert_eq!(by_mark.state, VerificationState::RegisteredButChanged);

    std::fs::write(&asset, &original).unwrap();
    provider.revoke(provider.key_id()).unwrap();
    let untrusted = provider.verify(&asset, Some(&manifest_bytes)).unwrap();
    assert_eq!(untrusted.state, VerificationState::MarkFoundClaimNotTrusted);
    assert_eq!(untrusted.reason, "signing key is revoked");
    assert_eq!(
        untrusted.normative_note,
        apw_core::NOTHING_FOUND_NORMATIVE_NOTE
    );
}

fn key_identifier<'a>(certificate: &'a X509Certificate<'a>, oid: &Oid<'_>) -> Vec<u8> {
    let extension = certificate
        .get_extension_unique(oid)
        .unwrap()
        .expect("extension present");
    match extension.parsed_extension() {
        ParsedExtension::AuthorityKeyIdentifier(aki) => {
            aki.key_identifier.as_ref().unwrap().0.to_vec()
        }
        ParsedExtension::SubjectKeyIdentifier(id) => id.0.to_vec(),
        other => panic!("unexpected extension: {other:?}"),
    }
}

/// A 16-bit mono PCM WAV whose loudness, zero-crossing rate and crest all change
/// across windows, so the quantised descriptor is not degenerate.
fn write_pcm_wav(path: &Path, seconds: f64) {
    let frames = (f64::from(SAMPLE_RATE) * seconds) as usize;
    let mut data = Vec::with_capacity(frames * 2);
    for index in 0..frames {
        let time = index as f64 / f64::from(SAMPLE_RATE);
        let frequency = 110.0 + 90.0 * (time * 0.7).sin();
        let amplitude = 0.15 + 0.6 * (0.5 + 0.5 * (time * 1.3).cos());
        let sample = (amplitude * (core::f64::consts::TAU * frequency * time).sin()
            * f64::from(i16::MAX)) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
    }

    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav).unwrap();
}

/// The software fallback's sealed blobs are authenticated before they are
/// decrypted. A flipped ciphertext byte must be refused, not returned as
/// plaintext.
#[test]
fn sealed_data_is_rejected_when_it_has_been_tampered_with() {
    use apw_provenance::{HardwareProvider, SoftwareProvider};

    let store = tempfile::tempdir().unwrap();
    let provider = SoftwareProvider::new(&store.path().join("device_key.bin")).unwrap();

    let secret = b"chain root for a session that must not leak";
    let sealed = provider.seal(secret).unwrap();
    assert_ne!(&sealed[32..], &secret[..], "plaintext must not be stored raw");
    assert_eq!(provider.unseal(&sealed).unwrap().as_slice(), secret);

    let mut tampered = sealed.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    assert!(provider.unseal(&tampered).is_err());

    let mut short = sealed.clone();
    short.truncate(31);
    assert!(provider.unseal(&short).is_err());

    let signature = provider.sign(secret).unwrap();
    assert!(provider.verify(secret, &signature).unwrap());
    assert!(!provider.verify(b"different data", &signature).unwrap());
}
