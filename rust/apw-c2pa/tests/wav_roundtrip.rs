use std::error::Error;
use std::fs;
use std::path::Path;

use apw_c2pa::{
    build_manifest, detect_format, sign_asset, sign_embedded, unobserved_ingredient, verify_asset,
    C2paSigner, Ingredient, IngredientRelationship, ManifestSpec, ObservedAction, SigningAlgorithm,
    SigningMode, ACTIONS_ASSERTION_LABEL, AIFF_MIME, UNOBSERVED_ASSERTION_LABEL, WAV_MIME,
};
use apw_core::{ProofLevel, VerificationState};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use time::{Duration, OffsetDateTime};

#[path = "../../test-support/audio.rs"]
mod audio;

type TestResult = Result<(), Box<dyn Error>>;

struct Chain {
    chain_pem: String,
    key_pem: String,
    root_pem: String,
}

// IMPORTANT: c2pa-rs rejects a signing certificate whose subject carries only a common name
// (it reports claimSignature.mismatch), so organizationName is required on both certificates.
fn distinguished_name(common_name: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, common_name);
    name.push(DnType::OrganizationName, "APW Test Signer");
    name.push(DnType::CountryName, "US");
    name
}

fn make_chain(label: &str) -> Result<Chain, Box<dyn Error>> {
    let now = OffsetDateTime::now_utc();
    let not_before = now - Duration::days(1);
    let not_after = now + Duration::days(365);

    let root_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut root_params = CertificateParams::new(Vec::<String>::new())?;
    root_params.distinguished_name = distinguished_name(&format!("{label} root"));
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root_params.use_authority_key_identifier_extension = true;
    root_params.not_before = not_before;
    root_params.not_after = not_after;
    let root = root_params.self_signed(&root_key)?;

    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut leaf_params = CertificateParams::new(Vec::<String>::new())?;
    leaf_params.distinguished_name = distinguished_name(&format!("{label} leaf"));
    leaf_params.is_ca = IsCa::ExplicitNoCa;
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    leaf_params.use_authority_key_identifier_extension = true;
    leaf_params.not_before = not_before;
    leaf_params.not_after = not_after;
    let issuer = Issuer::from_params(&root_params, &root_key);
    let leaf = leaf_params.signed_by(&leaf_key, &issuer)?;

    Ok(Chain {
        chain_pem: format!("{}{}", leaf.pem(), root.pem()),
        key_pem: leaf_key.serialize_pem(),
        root_pem: root.pem(),
    })
}

fn signer_for(chain: &Chain) -> Result<C2paSigner, Box<dyn Error>> {
    Ok(C2paSigner::from_pem(
        chain.chain_pem.as_bytes(),
        chain.key_pem.as_bytes(),
        SigningAlgorithm::Es256,
    )?)
}

fn pcm16_ramp(frames: usize) -> impl Iterator<Item = i16> {
    (0..frames).map(|index| (((index as i64 * 37) % 3000) - 1500) as i16)
}

fn float_ramp(frames: usize) -> impl Iterator<Item = f32> {
    (0..frames).map(|index| (index % 100) as f32 / 100.0)
}

fn write_wav_pcm16(path: &Path, frames: usize) -> TestResult {
    fs::write(path, audio::riff_wav(1, 1, 44100, 16, &pcm16_ramp(frames).flat_map(i16::to_le_bytes).collect::<Vec<u8>>()))?;
    Ok(())
}

fn write_wav_float32(path: &Path, frames: usize) -> TestResult {
    let data: Vec<u8> = float_ramp(frames).flat_map(f32::to_le_bytes).collect();
    fs::write(path, audio::riff_wav(3, 1, 44100, 32, &data))?;
    Ok(())
}

fn write_aiff(path: &Path, frames: usize) -> TestResult {
    let pcm: Vec<u8> = pcm16_ramp(frames).flat_map(i16::to_be_bytes).collect();
    fs::write(path, audio::aiff(b"AIFF", 1, frames as u32, 16, &[], &pcm))?;
    Ok(())
}

fn write_aifc_float32(path: &Path, frames: usize) -> TestResult {
    let data: Vec<u8> = float_ramp(frames).flat_map(f32::to_be_bytes).collect();
    fs::write(path, audio::aiff(b"AIFC", 1, frames as u32, 32, b"fl32", &data))?;
    Ok(())
}

fn spec_for(sample: &Path) -> Result<ManifestSpec, Box<dyn Error>> {
    let mut spec = ManifestSpec::new("mix.wav");
    spec.actions
        .push(ObservedAction::new("clip_paste", ProofLevel::Inferred).confidence(0.8));
    spec.ingredients.push(Ingredient::declared(
        "stem-1",
        IngredientRelationship::ComponentOf,
        ProofLevel::DirectlyObserved,
        "a".repeat(64),
    )?);
    spec.ingredients
        .push(unobserved_ingredient("sample.wav", sample)?);
    Ok(spec)
}

fn flip_byte(source: &Path, dest: &Path, offset: usize) -> TestResult {
    let mut payload = fs::read(source)?;
    let byte = payload
        .get_mut(offset)
        .ok_or("tamper offset is past the end of the asset")?;
    *byte ^= 0xFF;
    fs::write(dest, payload)?;
    Ok(())
}

#[test]
fn signed_wav_verifies_and_keeps_its_assertions_and_per_ingredient_proof_levels() -> TestResult {
    let dir = tempfile::tempdir()?;
    let chain = make_chain("apw")?;
    let sample = dir.path().join("sample.wav");
    write_wav_pcm16(&sample, 500)?;
    let src = dir.path().join("mix.wav");
    write_wav_pcm16(&src, 4000)?;
    let dest = dir.path().join("mix-signed.wav");

    let spec = spec_for(&sample)?;
    let result = sign_embedded(&src, &dest, &build_manifest(&spec)?, &signer_for(&chain)?)?;
    assert_eq!(result.mode, SigningMode::Embedded);

    let original = fs::read(&src)?;
    let signed = fs::read(&dest)?;
    assert!(signed.len() > original.len());
    let differing: Vec<usize> = original
        .iter()
        .zip(signed.iter())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| index)
        .collect();
    // Only the RIFF size field at offset 4 may differ; every audio byte is carried through.
    assert!(
        differing.iter().all(|index| (4..8).contains(index)),
        "unexpected rewritten bytes: {differing:?}"
    );
    let excluded = result
        .binding
        .excluded_region
        .ok_or("the embedded binding must record its excluded region")?;
    assert_eq!(excluded.start, original.len() as u64);
    assert_eq!(excluded.length, (signed.len() - original.len()) as u64);

    let verified = verify_asset(&dest, WAV_MIME, Some(&chain.root_pem), None)?;
    assert_eq!(
        verified.state,
        VerificationState::Verified,
        "{} {:?}",
        verified.detail,
        verified.failure_codes
    );
    assert!(verified.failure_codes.is_empty());
    assert!(verified.has_assertion(ACTIONS_ASSERTION_LABEL));
    assert!(verified.has_assertion(UNOBSERVED_ASSERTION_LABEL));

    // A Verified state proves nothing about content survival, so pin the ingredients too.
    let manifest = verified.manifest.ok_or("verified manifest is missing")?;
    let ingredients = manifest
        .get("ingredients")
        .and_then(|value| value.as_array())
        .ok_or("the signed manifest dropped its ingredients")?;
    assert_eq!(ingredients.len(), 2);
    let mut relationships: Vec<&str> = ingredients
        .iter()
        .filter_map(|ing| ing.get("relationship").and_then(|v| v.as_str()))
        .collect();
    relationships.sort_unstable();
    assert_eq!(relationships, vec!["componentOf", "inputTo"]);

    let unobserved = ingredients
        .iter()
        .find(|ing| ing.get("relationship").and_then(|v| v.as_str()) == Some("inputTo"))
        .ok_or("the unobserved ingredient did not survive signing")?;
    let metadata = unobserved
        .get("metadata")
        .ok_or("the unobserved ingredient lost its metadata")?;
    assert_eq!(
        metadata.get("apw:proof_level").and_then(|v| v.as_str()),
        Some("unknown_unobserved")
    );
    assert_eq!(
        metadata.get("apw:sha256").and_then(|v| v.as_str()),
        Some(apw_core::sha256_file(&sample)?.as_str())
    );
    Ok(())
}

#[test]
fn a_flipped_audio_byte_is_registered_but_changed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let chain = make_chain("apw")?;
    let sample = dir.path().join("sample.wav");
    write_wav_pcm16(&sample, 500)?;
    let src = dir.path().join("mix.wav");
    write_wav_pcm16(&src, 4000)?;
    let dest = dir.path().join("mix-signed.wav");
    let spec = spec_for(&sample)?;
    sign_embedded(&src, &dest, &build_manifest(&spec)?, &signer_for(&chain)?)?;

    let tampered = dir.path().join("mix-tampered.wav");
    flip_byte(&dest, &tampered, fs::metadata(&src)?.len() as usize / 2)?;

    let changed = verify_asset(&tampered, WAV_MIME, Some(&chain.root_pem), None)?;
    assert_eq!(changed.state, VerificationState::RegisteredButChanged);
    assert!(changed
        .failure_codes
        .iter()
        .any(|code| code == "assertion.dataHash.mismatch"));
    Ok(())
}

#[test]
fn a_chain_outside_the_anchor_list_is_mark_found_claim_not_trusted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let chain = make_chain("apw")?;
    let unrelated = make_chain("unrelated")?;
    let sample = dir.path().join("sample.wav");
    write_wav_pcm16(&sample, 500)?;
    let src = dir.path().join("mix.wav");
    write_wav_pcm16(&src, 4000)?;
    let dest = dir.path().join("mix-signed.wav");
    let spec = spec_for(&sample)?;
    sign_embedded(&src, &dest, &build_manifest(&spec)?, &signer_for(&chain)?)?;

    let untrusted = verify_asset(&dest, WAV_MIME, Some(&unrelated.root_pem), None)?;
    assert_eq!(untrusted.state, VerificationState::MarkFoundClaimNotTrusted);
    assert!(untrusted.trust_evaluated);
    assert!(untrusted
        .failure_codes
        .iter()
        .any(|code| code == "signingCredential.untrusted"));

    // No anchors at all is also not-trusted, never verified: the signer was never evaluated.
    let unevaluated = verify_asset(&dest, WAV_MIME, None, None)?;
    assert_eq!(
        unevaluated.state,
        VerificationState::MarkFoundClaimNotTrusted
    );
    assert!(!unevaluated.trust_evaluated);
    Ok(())
}

#[test]
fn thirty_two_bit_float_wav_is_refused_before_any_signing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let float_wav = dir.path().join("float.wav");
    write_wav_float32(&float_wav, 1000)?;

    let refusal = detect_format(&float_wav).err().ok_or(
        "a 32-bit float WAV must be refused: the stem/export association does not survive it",
    )?;
    let message = refusal.to_string();
    assert!(message.contains("32-bit float WAV"), "{message}");
    assert!(
        message.contains("Re-export as 16-bit PCM WAV"),
        "the refusal must name the format the user should export: {message}"
    );

    let chain = make_chain("apw")?;
    let sample = dir.path().join("sample.wav");
    write_wav_pcm16(&sample, 500)?;
    let spec = spec_for(&sample)?;
    let dest = dir.path().join("float-signed.wav");
    assert!(
        sign_embedded(&float_wav, &dest, &build_manifest(&spec)?, &signer_for(&chain)?).is_err()
    );
    assert!(!dest.exists());

    // The same refusal has to hold for the AIFC float encodings, which reach the sidecar path.
    let float_aifc = dir.path().join("float.aifc");
    write_aifc_float32(&float_aifc, 1000)?;
    let aifc_refusal = detect_format(&float_aifc)
        .err()
        .ok_or("an fl32 AIFC must be refused too")?
        .to_string();
    assert!(aifc_refusal.contains("32-bit float AIFF"), "{aifc_refusal}");
    assert!(aifc_refusal.contains("Re-export as 16-bit PCM WAV"), "{aifc_refusal}");
    Ok(())
}

#[test]
fn an_unsigned_wav_is_nothing_found_not_evidence_of_anything() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bare = dir.path().join("bare.wav");
    write_wav_pcm16(&bare, 4000)?;
    let outcome = verify_asset(&bare, WAV_MIME, None, None)?;
    assert_eq!(outcome.state, VerificationState::NothingFound);
    assert!(outcome.manifest.is_none());
    Ok(())
}

#[test]
fn aiff_routes_to_a_sidecar_whose_binding_has_no_exclusions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let chain = make_chain("apw")?;
    let sample = dir.path().join("sample.wav");
    write_wav_pcm16(&sample, 500)?;
    let src = dir.path().join("mix.aiff");
    write_aiff(&src, 4000)?;

    let mut spec = spec_for(&sample)?;
    spec.mime = AIFF_MIME.to_string();
    let result = sign_asset(
        &src,
        &dir.path().join("mix-signed.aiff"),
        &build_manifest(&spec)?,
        &signer_for(&chain)?,
        None,
    )?;
    assert_eq!(result.mode, SigningMode::Sidecar);
    let manifest_path = result
        .manifest_path
        .clone()
        .ok_or("the sidecar path must report where it wrote the manifest")?;
    assert_eq!(manifest_path.extension().and_then(|e| e.to_str()), Some("c2pa"));
    assert_eq!(result.binding.exclusions.as_deref(), Some(&[][..]));
    assert_eq!(result.binding.container_rewrite_tolerance, "none");
    assert!(result.binding.is_whole_file());

    let sidecar = fs::read(&manifest_path)?;
    // IMPORTANT: the sidecar hash covers `src` verbatim. Nothing was embedded, so the asset
    // c2pa-rs hashed is the untouched source file, not the copy it wrote to its scratch sink.
    let verified = verify_asset(&src, AIFF_MIME, Some(&chain.root_pem), Some(&sidecar))?;
    assert_eq!(
        verified.state,
        VerificationState::Verified,
        "{} {:?}",
        verified.detail,
        verified.failure_codes
    );

    // Zero exclusions means a flip anywhere - payload, the COMM header, or the last byte -
    // has to break the binding.
    let original_len = fs::metadata(&src)?.len() as usize;
    for offset in [original_len / 2, 20, original_len - 1] {
        let tampered = dir.path().join(format!("mix-tampered-{offset}.aiff"));
        flip_byte(&src, &tampered, offset)?;
        let changed = verify_asset(&tampered, AIFF_MIME, Some(&chain.root_pem), Some(&sidecar))?;
        assert_eq!(
            changed.state,
            VerificationState::RegisteredButChanged,
            "offset {offset} did not break the whole-file binding"
        );
        assert!(changed
            .failure_codes
            .iter()
            .any(|code| code == "assertion.dataHash.mismatch"));
    }
    Ok(())
}

#[test]
fn a_lone_self_signed_certificate_is_refused_as_a_signing_chain() -> TestResult {
    let chain = make_chain("apw")?;
    let leaf_only = chain
        .chain_pem
        .split_inclusive("-----END CERTIFICATE-----\n")
        .next()
        .ok_or("the generated chain is not PEM")?
        .to_string();

    let refusal = C2paSigner::from_pem(
        leaf_only.as_bytes(),
        chain.key_pem.as_bytes(),
        SigningAlgorithm::Es256,
    )
    .err()
    .ok_or("a single certificate must not be accepted as a signing chain")?
    .to_string();
    assert!(refusal.contains("at least one CA certificate"), "{refusal}");

    // The root alone is also not a signing chain, even though it is a CA.
    assert!(C2paSigner::from_pem(
        chain.root_pem.as_bytes(),
        chain.key_pem.as_bytes(),
        SigningAlgorithm::Es256
    )
    .is_err());
    Ok(())
}
