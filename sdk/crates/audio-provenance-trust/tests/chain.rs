//! What a chain must refuse. Each case here is an impersonation route, not a code path.

#![allow(clippy::unwrap_used, clippy::panic, clippy::string_slice)]

use audio_provenance_core::SigningKey;
use audio_provenance_trust::{
    Anchor, Capability, Instant, Refusal, RevocationEntry, RevocationList, RevocationReason,
    SignedAnchor, SignedRecord, SignerRecord, TrustEvaluation, TrustStore, Window,
};
use apw_trace::{TrustResolution, TrustStore as _};
use serde_json::Value;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_raw_bytes(&[seed; 32]).unwrap()
}

fn at(text: &str) -> Instant {
    Instant::parse(text).unwrap()
}

fn window(not_before: &str, not_after: &str) -> Window {
    Window::new(at(not_before), at(not_after)).unwrap()
}

const NOW: &str = "2026-08-31T12:00:00Z";

fn anchor(key: &SigningKey, max_chain_depth: u64) -> SignedAnchor {
    Anchor {
        anchor_id: "signal-room-ca".to_string(),
        name: "Signal Room Studios Authority".to_string(),
        public_key: key.public_key_bytes(),
        window: window("2026-01-01T00:00:00Z", "2031-01-01T00:00:00Z"),
        max_chain_depth,
    }
    .self_sign(key)
    .unwrap()
}

fn record(
    id: &str,
    subject: &SigningKey,
    name: &str,
    capability: Capability,
    issuer: &SigningKey,
    window: Window,
) -> SignedRecord {
    SignerRecord {
        record_id: id.to_string(),
        subject_public_key: subject.public_key_bytes(),
        display_name: name.to_string(),
        capability,
        issuer_anchor_id: "signal-room-ca".to_string(),
        issuer_public_key: issuer.public_key_bytes(),
        window,
    }
    .sign(issuer)
    .unwrap()
}

fn live() -> Window {
    window("2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z")
}

fn store(anchor: SignedAnchor, records: Vec<SignedRecord>) -> TrustStore {
    let mut store = TrustStore::new();
    store.insert_anchor(anchor).unwrap();
    for record in records {
        store.insert_record(record).unwrap();
    }
    store
}

#[test]
fn a_chain_to_a_trusted_anchor_names_the_signer_and_the_anchor() {
    let authority = key(1);
    let signer = key(2);
    let store = store(
        anchor(&authority, 2),
        vec![record(
            "signal-room-master",
            &signer,
            "Signal Room Studios",
            Capability::Leaf,
            &authority,
            live(),
        )],
    );

    let TrustEvaluation::Vouched(identity) = store.evaluate(&signer.public_key_bytes(), &at(NOW))
    else {
        panic!("a valid chain must resolve");
    };
    assert_eq!(identity.display_name, "Signal Room Studios");
    assert_eq!(identity.anchor_id, "signal-room-ca");
    assert_eq!(identity.anchor_key_id, authority.signer_id());
    assert_eq!(identity.chain_depth, 1);
    // The name is never reported without the anchor that vouched for it.
    assert!(identity.authority().contains("signal-room-ca"));
    assert!(identity.authority().contains(&identity.anchor_key_id));
}

/// The record binds a name to 32 key bytes. Substituting the subject key of an otherwise valid
/// record is the whole impersonation, so the issuer's signature must be what refuses it, never the
/// truncated signer id.
#[test]
fn a_substituted_subject_key_is_refused() {
    let authority = key(1);
    let signer = key(2);
    let attacker = key(3);
    let genuine = record(
        "signal-room-master",
        &signer,
        "Signal Room Studios",
        Capability::Leaf,
        &authority,
        live(),
    );

    let mut document = genuine.to_value();
    let object = document.as_object_mut().unwrap();
    object.insert(
        "subject_public_key_hex".to_string(),
        Value::String(hex::encode(attacker.public_key_bytes())),
    );
    object.insert(
        "subject_signer_id".to_string(),
        Value::String(attacker.signer_id()),
    );

    // The document is internally consistent; only the issuer's signature disagrees.
    let forged = SignedRecord::parse(&document).unwrap();
    let store = store(anchor(&authority, 2), vec![forged]);
    assert_eq!(
        store.evaluate(&attacker.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Refused(Refusal::RecordSignatureInvalid {
            record_id: "signal-room-master".to_string(),
        })
    );
}

/// A leaf holds a name and nothing else. If a leaf could issue, every named signer would be an
/// authority able to mint any other name under the same anchor.
#[test]
fn a_leaf_record_may_not_issue_another_record() {
    let authority = key(1);
    let named = key(2);
    let victim = key(3);
    let store = store(
        anchor(&authority, 4),
        vec![
            record(
                "named-leaf",
                &named,
                "Signal Room Studios",
                Capability::Leaf,
                &authority,
                live(),
            ),
            record(
                "issued-by-a-leaf",
                &victim,
                "Someone Else Entirely",
                Capability::Leaf,
                &named,
                live(),
            ),
        ],
    );
    assert_eq!(
        store.evaluate(&victim.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Refused(Refusal::IssuerNotPermitted {
            record_id: "issued-by-a-leaf".to_string(),
            issuer_key_id: named.signer_id(),
        })
    );
}

#[test]
fn a_chain_deeper_than_the_anchor_policy_is_refused() {
    let authority = key(1);
    let intermediate = key(2);
    let signer = key(3);
    let records = vec![
        record(
            "intermediate",
            &intermediate,
            "Signal Room Mastering",
            Capability::Issuer,
            &authority,
            live(),
        ),
        record(
            "leaf",
            &signer,
            "Signal Room Studios",
            Capability::Leaf,
            &intermediate,
            live(),
        ),
    ];

    let permitted = store(anchor(&authority, 2), records.clone());
    let TrustEvaluation::Vouched(identity) =
        permitted.evaluate(&signer.public_key_bytes(), &at(NOW))
    else {
        panic!("depth 2 is inside a policy of 2");
    };
    assert_eq!(identity.chain_depth, 2);

    let refused = store(anchor(&authority, 1), records);
    assert_eq!(
        refused.evaluate(&signer.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Refused(Refusal::ChainTooDeep { limit: 1 })
    );
}

/// The ceiling holds even when an anchor's own policy asks for more than the build permits.
#[test]
fn the_absolute_depth_ceiling_bounds_the_walk() {
    let authority = key(1);
    let mut records = Vec::new();
    let mut issuer = key(1);
    for step in 0..audio_provenance_trust::CHAIN_DEPTH_CEILING {
        let subject = key(u8::try_from(step).unwrap() + 10);
        records.push(record(
            &format!("issuer-{step}"),
            &subject,
            "Intermediate",
            Capability::Issuer,
            &issuer,
            live(),
        ));
        issuer = subject;
    }
    let leaf_key = key(200);
    records.push(record(
        "leaf",
        &leaf_key,
        "Signal Room Studios",
        Capability::Leaf,
        &issuer,
        live(),
    ));

    let store = store(
        anchor(&authority, audio_provenance_trust::CHAIN_DEPTH_CEILING),
        records,
    );
    assert_eq!(
        store.evaluate(&leaf_key.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Refused(Refusal::ChainTooDeep {
            limit: audio_provenance_trust::CHAIN_DEPTH_CEILING,
        })
    );
}

/// Revocation names the signer it refuses and states that it is retroactive, so a consumer reading
/// the output learns both who was revoked and that an earlier claimed signing time will not help.
#[test]
fn a_revoked_signer_is_refused_and_named() {
    let authority = key(1);
    let signer = key(2);
    let mut store = store(
        anchor(&authority, 2),
        vec![record(
            "signal-room-master",
            &signer,
            "Signal Room Studios",
            Capability::Leaf,
            &authority,
            live(),
        )],
    );
    store
        .insert_revocations(
            RevocationList {
                anchor_id: "signal-room-ca".to_string(),
                issuer_public_key: authority.public_key_bytes(),
                issued_at: at("2026-08-30T00:00:00Z"),
                entries: vec![RevocationEntry {
                    subject_public_key: signer.public_key_bytes(),
                    revoked_at: at("2026-08-30T00:00:00Z"),
                    reason: RevocationReason::KeyCompromise,
                }],
            }
            .sign(&authority)
            .unwrap(),
        )
        .unwrap();

    let TrustEvaluation::Refused(refusal) = store.evaluate(&signer.public_key_bytes(), &at(NOW))
    else {
        panic!("a revoked signer must be refused");
    };
    assert_eq!(refusal.code(), "signer_revoked");
    let described = refusal.describe();
    assert!(described.contains("Signal Room Studios"), "{described}");
    assert!(described.contains("key_compromise"), "{described}");
    assert!(described.contains("retroactive"), "{described}");

    // Evaluating at an instant BEFORE the revocation still refuses. The rule is one rule.
    assert!(matches!(
        store.evaluate(&signer.public_key_bytes(), &at("2026-02-01T00:00:00Z")),
        TrustEvaluation::Refused(Refusal::Revoked { .. })
    ));
}

/// A list signed by anything but the anchor it names cannot take a name away.
#[test]
fn a_revocation_list_from_a_foreign_key_is_inert() {
    let authority = key(1);
    let signer = key(2);
    let impostor = key(9);
    let mut store = store(
        anchor(&authority, 2),
        vec![record(
            "signal-room-master",
            &signer,
            "Signal Room Studios",
            Capability::Leaf,
            &authority,
            live(),
        )],
    );
    store
        .insert_revocations(
            RevocationList {
                anchor_id: "signal-room-ca".to_string(),
                issuer_public_key: impostor.public_key_bytes(),
                issued_at: at("2026-08-30T00:00:00Z"),
                entries: vec![RevocationEntry {
                    subject_public_key: signer.public_key_bytes(),
                    revoked_at: at("2026-08-30T00:00:00Z"),
                    reason: RevocationReason::KeyCompromise,
                }],
            }
            .sign(&impostor)
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        store.evaluate(&signer.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Vouched(_)
    ));
}

#[test]
fn an_expired_record_is_refused_at_the_verification_instant() {
    let authority = key(1);
    let signer = key(2);
    let store = store(
        anchor(&authority, 2),
        vec![record(
            "signal-room-master",
            &signer,
            "Signal Room Studios",
            Capability::Leaf,
            &authority,
            window("2026-01-01T00:00:00Z", "2026-06-01T00:00:00Z"),
        )],
    );
    assert_eq!(
        store.evaluate(&signer.public_key_bytes(), &at(NOW)),
        TrustEvaluation::Refused(Refusal::RecordExpired {
            record_id: "signal-room-master".to_string(),
            not_after: at("2026-06-01T00:00:00Z"),
        })
    );
    assert!(matches!(
        store.evaluate(&signer.public_key_bytes(), &at("2026-03-01T00:00:00Z")),
        TrustEvaluation::Vouched(_)
    ));
}

/// The anchor is the root of everything else, so a flipped byte in its self-signature has to stop
/// the document at parse rather than survive into a store.
#[test]
fn a_tampered_anchor_self_signature_never_parses() {
    let authority = key(1);
    let mut document = anchor(&authority, 2).to_value();
    let object = document.as_object_mut().unwrap();
    let signature = object
        .get("self_signature_hex")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    let flipped = format!(
        "{}{}",
        if signature.starts_with('0') { '1' } else { '0' },
        &signature[1..]
    );
    object.insert("self_signature_hex".to_string(), Value::String(flipped));
    assert!(SignedAnchor::parse(&document).is_err());

    // A renamed anchor carrying its original signature is the same refusal.
    let mut renamed = anchor(&authority, 2).to_value();
    renamed.as_object_mut().unwrap().insert(
        "name".to_string(),
        Value::String("Someone Else".to_string()),
    );
    assert!(SignedAnchor::parse(&renamed).is_err());
}

/// Hand-written, not produced by this crate's writer: a field outside the signed set would be a
/// field an issuer never signed, and a display name reaches a terminal verbatim.
#[test]
fn a_document_outside_the_signed_grammar_is_refused() {
    let authority = key(1);
    let mut document = anchor(&authority, 2).to_value();
    document
        .as_object_mut()
        .unwrap()
        .insert("policy_url".to_string(), Value::String("x".to_string()));
    assert!(SignedAnchor::parse(&document).is_err());

    // Text that displays as something other than what was signed: a terminal-clearing control
    // sequence, a right-to-left override, and a zero-width joiner. All refused before the
    // signature is even reached, because a vouched name is what this crate exists to print.
    for injected in [
        format!("Signal Room{}[2J Studios", char::from(27)),
        format!("Signal Room{}soidutS", char::from_u32(0x202E).unwrap()),
        format!("Signal{}Room Studios", char::from_u32(0x200D).unwrap()),
        format!("Signal Room Studios{}", char::from_u32(0xFEFF).unwrap()),
    ] {
        let mut escaped = anchor(&authority, 2).to_value();
        escaped
            .as_object_mut()
            .unwrap()
            .insert("name".to_string(), Value::String(injected.clone()));
        let error = SignedAnchor::parse(&escaped).unwrap_err();
        assert_eq!(
            audio_provenance_core::CodedError::code(&error),
            "trust_field_unprintable",
            "accepted {:?}: {error}",
            injected.escape_unicode().to_string()
        );
    }
}
/// The adapter is the only thing `apw_trace` sees, and `Anchored` is the only input from which its
/// status mapping produces `externally_verified`. Nothing else it can return may be `Anchored`.
#[test]
fn the_adapter_reports_anchored_only_for_a_completed_chain() {
    let authority = key(1);
    let signer = key(2);
    let stranger = key(7);
    let bound = audio_provenance_trust::AnchoredTrustStore::new(
        store(
            anchor(&authority, 2),
            vec![record(
                "signal-room-master",
                &signer,
                "Signal Room Studios",
                Capability::Leaf,
                &authority,
                live(),
            )],
        ),
        at(NOW),
    );

    let manifest = serde_json::json!({"a": 1});
    let proof = |key: &SigningKey| {
        let signature = key.sign_manifest(&manifest, "signer.pub").unwrap();
        audio_provenance_core::verify_manifest_signature(&manifest, &signature, None).unwrap()
    };

    match bound.resolve(&proof(&signer)) {
        TrustResolution::Anchored(anchor) => {
            assert_eq!(anchor.identity, "Signal Room Studios");
            assert!(anchor.authority.contains("signal-room-ca"), "{anchor:?}");
        }
        other => panic!("expected an anchored resolution, found {other:?}"),
    }
    assert!(matches!(
        bound.resolve(&proof(&stranger)),
        TrustResolution::Unanchored
    ));
}
