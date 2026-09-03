#![cfg(feature = "local")]
#![allow(clippy::panic)]
#![allow(clippy::unwrap_used)]

use std::path::Path;

use audio_provenance_core::SigningKey;
use audio_provenance_registry::config::MapEnv;
use audio_provenance_registry::{
    ContentHash, Fingerprint, LocalRegistryBackend, Lookup, MarkId, RecordId, RegistryBackend,
    RegistryConfig, RegistryError, RegistryRecord, RegistryResolver, SignedAt, UnavailableKind,
};

const KEY: [u8; 32] = [7u8; 32];

/// Distinct salts, because distinct works get distinct locators. Two fixtures sharing one is the
/// collision case, and `a_second_record_may_not_take_an_occupied_locator` is where that belongs.
fn record(note: &str, content_byte: u8) -> RegistryRecord {
    record_with_salt(
        note,
        content_byte,
        &format!("{content_byte:02x}").repeat(16),
    )
}

fn record_with_salt(note: &str, content_byte: u8, salt: &str) -> RegistryRecord {
    let key = SigningKey::from_raw_bytes(&KEY).unwrap();
    // The salt is inside the SIGNED view: a record whose locator could be edited after signing
    // would be one whose mark id the signer never chose.
    let unsigned = serde_json::json!({
        "apw_version": "0.9.0",
        "schema": "audio-provenance-manifest-v0",
        "created_at": "2026-08-30T02:19:26.350552Z",
        "locator_salt": salt,
        "note": note,
    });
    let signature = key
        .sign_manifest(&unsigned, "/keys/demo_ed25519_public.key")
        .unwrap();
    let mut manifest = unsigned;
    manifest["portable_signature"] = serde_json::to_value(&signature).unwrap();

    RegistryRecord::from_signed_manifest(
        manifest,
        ContentHash::from_content(&[content_byte; 64]),
        Some(Fingerprint::new(vec![content_byte; 32]).unwrap()),
        SignedAt::parse("2026-08-30T02:19:26.350552Z").unwrap(),
    )
    .unwrap()
}

/// First writer wins a locator. A second record under an occupied one could only ever reach
/// `ambiguous_binding`, so admitting it would trade one working mark for two broken ones.
#[test]
fn a_second_record_may_not_take_an_occupied_locator() {
    let dir = tempfile::tempdir().unwrap();
    let registry = LocalRegistryBackend::init("local", dir.path()).unwrap();
    let salt = "cd".repeat(16);
    let held = record_with_salt("held", 8, &salt);
    let squatter = record_with_salt("squatter", 9, &salt);
    assert_eq!(held.mark_id(), squatter.mark_id());

    registry.put(&held).unwrap();
    let outcome = registry.put(&squatter);
    assert!(
        matches!(outcome, Err(RegistryError::LocatorConflict { .. })),
        "{outcome:?}"
    );
    // Idempotent re-publication of the record that holds it is still accepted.
    registry.put(&held).unwrap();

    let Lookup::Found(matches) = registry.lookup_by_mark(&held.mark_id()) else {
        panic!("the first writer's record must still resolve");
    };
    assert_eq!(matches.len(), 1);
}

/// A manifest with no locator_salt names no locator, so no mark could ever resolve it. Storing one
/// would put a row in the index that nothing can reach.
#[test]
fn a_manifest_without_a_locator_salt_cannot_be_stored() {
    let key = SigningKey::from_raw_bytes(&KEY).unwrap();
    let unsigned = serde_json::json!({
        "apw_version": "0.9.0",
        "schema": "audio-provenance-manifest-v0",
        "created_at": "2026-08-30T02:19:26.350552Z",
    });
    let signature = key.sign_manifest(&unsigned, "demo.key").unwrap();
    let mut manifest = unsigned;
    manifest["portable_signature"] = serde_json::to_value(&signature).unwrap();

    let outcome = RegistryRecord::from_signed_manifest(
        manifest,
        ContentHash::from_content(b"unreachable"),
        None,
        SignedAt::parse("2026-08-30T02:19:26.350552Z").unwrap(),
    );
    assert!(
        matches!(outcome, Err(RegistryError::MissingLocatorSalt)),
        "{outcome:?}"
    );
}

fn absent_mark() -> MarkId {
    MarkId::new(1, 0, [0xab; 6]).unwrap()
}

fn absent_content() -> ContentHash {
    ContentHash::from_content(b"nothing in any registry hashes to this")
}

/// The one that matters: a backend that cannot answer must not answer "no".
///
/// The key queried below is genuinely absent, which is where an implementation that folds an
/// outage into a miss still looks correct.
#[test]
fn an_outage_on_a_genuinely_absent_key_is_not_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let registry = LocalRegistryBackend::init("local", dir.path()).unwrap();
    let stored = record("stored", 1);
    registry.put(&stored).unwrap();

    assert!(
        registry
            .lookup_by_content_hash(&absent_content())
            .is_not_found(),
        "an intact registry must report a genuine absence as NotFound"
    );

    std::fs::write(dir.path().join("index.json"), b"{ truncated").unwrap();
    let reopened = LocalRegistryBackend::open("local", dir.path()).unwrap();
    for outcome in [
        reopened
            .lookup_by_content_hash(&absent_content())
            .map(|_| ()),
        reopened.lookup_by_mark(&absent_mark()).map(|_| ()),
        reopened.fetch(&stored.record_id()).map(|_| ()),
    ] {
        let reason = outcome.unavailable_reason().unwrap_or_else(|| {
            panic!("a corrupt index answered {outcome:?} instead of Unavailable")
        });
        assert_eq!(reason.kind(), UnavailableKind::IndexCorrupt);
    }
}

#[test]
fn an_index_entry_pointing_outside_the_root_is_refused_and_reported_as_an_outage() {
    let dir = tempfile::tempdir().unwrap();
    let registry = LocalRegistryBackend::init("local", dir.path()).unwrap();
    let stored = record("stored", 2);
    registry.put(&stored).unwrap();

    let secret = tempfile::tempdir().unwrap();
    std::fs::write(secret.path().join("stolen.json"), b"{}").unwrap();

    let index_path = dir.path().join("index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
    let hostile = format!(
        "../{}/stolen.json",
        secret.path().file_name().unwrap().to_str().unwrap()
    );
    index["entries"][0]["path"] = serde_json::Value::String(hostile);
    std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();

    let reopened = LocalRegistryBackend::open("local", dir.path()).unwrap();
    let outcome = reopened.fetch(&stored.record_id());
    let reason = outcome
        .unavailable_reason()
        .unwrap_or_else(|| panic!("a traversal entry answered {outcome:?}"));
    assert_eq!(reason.kind(), UnavailableKind::IndexCorrupt);
    assert!(
        reason.detail().contains("not permitted") || reason.detail().contains("outside"),
        "unexpected detail: {}",
        reason.detail()
    );

    for hostile in [
        "records/../../etc/passwd",
        "/etc/passwd",
        "records/x.json.part",
    ] {
        let mut index: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
        index["entries"][0]["path"] = serde_json::Value::String(hostile.into());
        std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
        let reopened = LocalRegistryBackend::open("local", dir.path()).unwrap();
        assert!(
            reopened.fetch(&stored.record_id()).is_unavailable(),
            "{hostile} was not refused"
        );
    }
}

#[test]
fn an_interrupted_index_write_leaves_the_previous_index_intact() {
    let dir = tempfile::tempdir().unwrap();
    let registry = LocalRegistryBackend::init("local", dir.path()).unwrap();
    let first = record("first", 3);
    registry.put(&first).unwrap();

    let index_path = dir.path().join("index.json");
    let committed = std::fs::read(&index_path).unwrap();

    // The on-disk state an interruption between the two writes leaves behind: the record
    // document landed, the index rename never happened, and a stage file is still lying around.
    let second = record("second", 4);
    let orphan = dir
        .path()
        .join("records")
        .join(format!("{}.json", second.record_id().to_hex()));
    std::fs::write(&orphan, second.to_envelope_bytes().unwrap()).unwrap();
    std::fs::write(
        index_path.with_extension("json.99-0.part"),
        b"{\"index_format\":\"audio-provenance-registry-index-v0\",\"entries\":[",
    )
    .unwrap();

    let reopened = LocalRegistryBackend::open("local", dir.path()).unwrap();
    assert_eq!(std::fs::read(&index_path).unwrap(), committed);
    assert!(matches!(
        reopened.fetch(&first.record_id()),
        Lookup::Found(_)
    ));
    assert!(
        reopened.fetch(&second.record_id()).is_not_found(),
        "an uncommitted record must not be reachable"
    );

    // And the registry still accepts writes afterwards.
    reopened.put(&second).unwrap();
    assert!(matches!(
        reopened.fetch(&second.record_id()),
        Lookup::Found(_)
    ));
    assert!(matches!(
        reopened.fetch(&first.record_id()),
        Lookup::Found(_)
    ));
}

#[test]
fn a_local_round_trip_preserves_the_bytes_every_key_is_derived_from() {
    let dir = tempfile::tempdir().unwrap();
    let registry = LocalRegistryBackend::init("local", dir.path()).unwrap();
    let stored = record("round trip", 5);
    registry.put(&stored).unwrap();

    let reopened = LocalRegistryBackend::open("local", dir.path()).unwrap();
    let Lookup::Found(found) = reopened.lookup_by_content_hash(&stored.content_hash()) else {
        panic!("stored record was not found by content hash");
    };
    assert_eq!(found.manifest_bytes(), stored.manifest_bytes());
    assert_eq!(
        RecordId::from_manifest_bytes(found.manifest_bytes()),
        stored.record_id()
    );
    assert_eq!(found.mark_id(), stored.mark_id());
    found.verify_key_possession().unwrap();

    let Lookup::Found(matches) = reopened.lookup_by_mark(&stored.mark_id()) else {
        panic!("stored record was not found by mark");
    };
    assert!(!matches.is_ambiguous());
    assert_eq!(matches.first().record_id(), stored.record_id());

    let Lookup::Found(candidates) =
        reopened.nearest_by_fingerprint(&Fingerprint::new(vec![5u8; 32]).unwrap(), 4)
    else {
        panic!("fingerprint search returned nothing");
    };
    assert_eq!(candidates.len(), 1);
    assert!(
        (candidates[0].advisory_score().advisory_value() - 1.0).abs() < f32::EPSILON,
        "an identical fingerprint should agree on every bit"
    );
}

#[test]
fn an_unconfigured_public_registry_names_what_to_set_and_guesses_no_url() {
    let env = MapEnv::default();
    let resolver = RegistryResolver::new(
        RegistryConfig::default(),
        Path::new("/tmp/audio-provenance.config.json").to_path_buf(),
        &env,
    );
    let error = resolver.resolve("public").unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("AUDIO_PROVENANCE_REGISTRY_URL"),
        "{message}"
    );
    assert!(
        message.contains("/tmp/audio-provenance.config.json"),
        "{message}"
    );
    assert!(
        !message.contains("the product website") && !message.contains("https://"),
        "the error suggested an endpoint: {message}"
    );
}

#[test]
fn the_environment_outranks_the_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    LocalRegistryBackend::init("local", dir.path()).unwrap();
    let ignored = tempfile::tempdir().unwrap();
    LocalRegistryBackend::init("local", ignored.path()).unwrap();

    let mut config = RegistryConfig::default();
    config
        .insert(
            "local",
            audio_provenance_registry::RegistryEntry::Local {
                root: ignored.path().to_path_buf(),
            },
        )
        .unwrap();

    let env = MapEnv::from_pairs([(
        audio_provenance_registry::REGISTRY_ROOT_ENV,
        dir.path().display().to_string(),
    )]);
    let resolver = RegistryResolver::new(
        config,
        dir.path().join("audio-provenance.config.json"),
        &env,
    );
    let backend = resolver.resolve("local").unwrap();
    assert_eq!(
        Path::new(backend.source().location()),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

#[test]
fn identifiers_reject_the_spellings_that_would_collide_on_disk() {
    assert!(matches!(
        MarkId::parse_hex("01A41F9C2E5B07"),
        Err(RegistryError::HexCharset { .. })
    ));
    assert!(matches!(
        MarkId::parse_hex("01a41f9c2e5b0"),
        Err(RegistryError::HexLength { .. })
    ));
    let mark = MarkId::new(1, 0, [0xa4, 0x1f, 0x9c, 0x2e, 0x5b, 0x07]).unwrap();
    assert_eq!(mark.to_hex(), "01a41f9c2e5b07");
    assert_eq!(MarkId::parse_hex(&mark.to_hex()).unwrap(), mark);
    assert!(MarkId::new(16, 0, [0; 6]).is_err());

    assert!(SignedAt::parse("2026-08-30T02:19:26Z").is_ok());
    assert!(SignedAt::parse("2026-08-30T02:19:26.350552Z").is_ok());
    assert!(SignedAt::parse("2026-08-30T02:19:26+00:00").is_err());
    assert!(SignedAt::parse("2026-13-30T02:19:26Z").is_err());
    assert!(SignedAt::parse("+026-08-30T02:19:26Z").is_err());
}
