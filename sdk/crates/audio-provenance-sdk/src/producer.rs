//! The producer surface: sign a record, embed a GenoMark-Q mark, publish to a registry.
//!
//! # The order is fixed, and it is not a convenience
//!
//! Embed, then hash, then sign. Marking changes the audio a hard binding covers, so a mark applied
//! after signing invalidates the record that was just written; [`embed`] refuses an asset that
//! already carries a manifest with [`SdkError::MarkAfterSign`].
//!
//! # A locator is allocated first, and the manifest commits to it
//!
//! The mark still resolves only to a record its own publisher intended, but the commitment runs the
//! other way. A [`MarkPlan`] rolls a 16-byte `locator_salt` and derives the 48-bit locator as
//! `sha256("audio-provenance-locator-v1" || 0x00 || signer public key || salt)`, which depends on no audio
//! and no manifest, so it exists before anything is embedded. [`sign`] then writes that salt into
//! the REQUIRED signed `locator_salt` field of the record it seals over the MARKED audio, and
//! [`publish`] stores the record under the locator derived from the manifest's own key and salt.
//!
//! GenoTrace re-derives the same value from the bytes a registry returns and refuses a mismatch as
//! `untrusted / locator_mismatch`, terminal, no descent. Binding the signer's public key into the
//! preimage is what makes squatting somebody else's locator a 2^48 search rather than a copy of
//! their published salt.
//!
//! One pass now completes: allocate, embed, hash, sign, publish. The circularity that made the mark
//! rung unreachable was the old derivation, not the property it protected.

use std::path::{Path, PathBuf};

use audio_provenance_audio::BitDepth;
use audio_provenance_core::{
    AssociationClaim, LOCATOR_SALT_BYTES, LocatorSalt, ManifestSigner, ObservationCoverage,
    derive_locator, parse_signing_input,
};
use audio_provenance_manifest::{HardBinding, ManifestDraft, MarkBinding, ProvenanceClaim};
use audio_provenance_registry::config::ProcessEnv;
use audio_provenance_registry::{
    ContentHash, LocalRegistryBackend, RecordId, RegistryKind, RegistryRecord, RegistryResolver,
    SignedAt, WritableRegistryBackend,
};
use genomark::{GenoMark, Payload};
use genotrace::container::{EmbeddedOutcome, find_embedded};
use genotrace::{IngestLimits, Ingested, ingest_path};
use serde::Serialize;

use crate::error::SdkError;
use crate::riff;

/// Where the signed record is written alongside the asset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SidecarOutput {
    /// `<asset>.audio-provenance.json`, the first path GenoTrace's sidecar rung reads.
    #[default]
    Conventional,
    Explicit(PathBuf),
    None,
}

#[derive(Debug)]
pub struct SignOptions {
    signer: Box<dyn ManifestSigner>,
    public_key_file: String,
    signed_at: SignedAt,
    sidecar: SidecarOutput,
    embed_manifest: bool,
    out: Option<PathBuf>,
    mark: Option<MarkBinding>,
    locator_salt: LocatorSalt,
    claims: Vec<ProvenanceClaim>,
    association: Option<AssociationClaim>,
    coverage: Option<ObservationCoverage>,
    limits: IngestLimits,
}

impl SignOptions {
    /// `signed_at` is an RFC 3339 UTC instant. The manifest publishes its calendar-date prefix and
    /// the registry stores the instant, so both forms come from one validated value.
    pub fn new(signer: impl ManifestSigner + 'static, signed_at: &str) -> Result<Self, SdkError> {
        Ok(Self {
            signer: Box::new(signer),
            public_key_file: String::new(),
            signed_at: SignedAt::parse(signed_at)?,
            sidecar: SidecarOutput::Conventional,
            embed_manifest: false,
            out: None,
            mark: None,
            locator_salt: fresh_salt()?,
            claims: Vec::new(),
            association: None,
            coverage: None,
            limits: IngestLimits::default(),
        })
    }

    /// The public-key file name recorded in the signature block, for a verifier that wants to find
    /// the key on disk. It is a label: the key that verifies is the one carried in the block.
    #[must_use]
    pub fn public_key_file(mut self, name: impl Into<String>) -> Self {
        self.public_key_file = name.into();
        self
    }

    #[must_use]
    pub fn sidecar(mut self, sidecar: SidecarOutput) -> Self {
        self.sidecar = sidecar;
        self
    }

    /// Writes the signed record into the output WAV's `aprv` chunk. Requires [`Self::out`].
    #[must_use]
    pub const fn embed_manifest(mut self, embed: bool) -> Self {
        self.embed_manifest = embed;
        self
    }

    #[must_use]
    pub fn out(mut self, path: impl Into<PathBuf>) -> Self {
        self.out = Some(path.into());
        self
    }

    /// Records the mark the asset was JUST embedded with, and the salt its locator came from.
    ///
    /// IMPORTANT: refuses a plan allocated under a DIFFERENT signing key. Signing with the wrong
    /// key would publish a record under a locator no mark points at, and the mark would resolve
    /// nothing with no error anywhere to say why.
    pub fn with_mark_plan(mut self, plan: &MarkPlan) -> Result<Self, SdkError> {
        let expected = derive_locator(&self.signer.public_key_bytes(), &plan.salt);
        let found = plan.payload.locator_bytes();
        if expected != found {
            return Err(SdkError::LocatorKeyMismatch {
                expected: hex::encode(expected),
                found: hex::encode(found),
            });
        }
        self.mark = Some(MarkBinding {
            version: plan.payload.version(),
            namespace: plan.payload.namespace(),
        });
        self.locator_salt = plan.salt;
        Ok(self)
    }

    /// Pins the salt instead of rolling one, for a deterministic test vector.
    #[must_use]
    pub const fn with_locator_salt(mut self, salt: LocatorSalt) -> Self {
        self.locator_salt = salt;
        self
    }

    #[must_use]
    pub fn with_claim(mut self, claim: ProvenanceClaim) -> Self {
        self.claims.push(claim);
        self
    }

    /// Carries a capture system's already-derived association without changing its proof level.
    #[must_use]
    pub const fn with_association(mut self, association: AssociationClaim) -> Self {
        self.association = Some(association);
        self
    }

    /// Carries a capture system's already-derived coverage without changing its proof level.
    #[must_use]
    pub const fn with_coverage(mut self, coverage: ObservationCoverage) -> Self {
        self.coverage = Some(coverage);
        self
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: IngestLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignResult {
    /// EXACTLY the canonical bytes that were signed and that the record digest covers.
    #[serde(skip)]
    pub manifest_bytes: Vec<u8>,
    /// SHA-256 over [`Self::manifest_bytes`]: the registry `RecordId`.
    pub record_id: String,
    /// The 48-bit registry key this record answers to, derived from the signer's public key and
    /// [`Self::locator_salt`]. A GenoMark payload carrying it resolves this record.
    pub locator: String,
    /// The 16 CSPRNG bytes the locator was derived from, as they appear in the signed manifest.
    pub locator_salt: String,
    pub signer_id: String,
    pub public_key_hex: String,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub decoded_audio_sha256: String,
    /// The RFC 3339 instant the registry stores.
    pub signed_at: String,
    /// The `YYYY-MM-DD` the manifest and a verifier publish.
    pub signed_date: String,
    pub mark: Option<MarkBinding>,
    /// The signed reference constellation's hex length, or `None` when the work was too long to
    /// carry one. A record without it can never reach `verified` over a lossy path, so its absence
    /// is reported rather than left for the verifier to discover.
    pub reference_fingerprint_hex_len: Option<usize>,
    pub sidecar_path: Option<String>,
    pub embedded_in: Option<String>,
    pub asset_path: String,
    /// False when the record was written into the asset itself.
    ///
    /// `content_sha256` is a straight hash over the file bytes, and the manifest carrying that
    /// digest is inside the bytes being hashed, so it cannot recompute over an asset with an
    /// embedded record. `decoded_audio_sha256` still recomputes exactly; GenoTrace reports
    /// `hard_exact_decoded_audio_only` plus a `container_bytes_changed` finding. Only the sidecar
    /// path leaves `content_sha256` exact.
    pub asset_binds_content_sha256: bool,
}

/// Hashes an asset and signs a Audio Provenance record over it.
pub fn sign(path: &Path, options: &SignOptions) -> Result<SignResult, SdkError> {
    let ingested = ingest_path(path, options.limits)?;
    let asset = options.out.as_deref().unwrap_or(path);

    if options.embed_manifest && options.out.is_none() {
        return Err(SdkError::InvalidOption {
            option: "embed_manifest",
            reason: "an output path is required; a record is never written into the input in place"
                .to_string(),
        });
    }

    let decoded_audio_sha256 = ingested.decoded_audio_sha256();
    let binding = HardBinding::new(
        ingested.content_sha256(),
        Some(ingested.content_bytes()),
        Some(&decoded_audio_sha256),
    )?;
    let signed_date = options.signed_at.date().to_string();

    let mut draft =
        ManifestDraft::new(&signed_date, binding)?.with_locator_salt(options.locator_salt);
    if let Some(mark) = options.mark {
        draft = draft.with_mark(mark);
    }
    if let Some(association) = options.association {
        draft = draft.with_association(association);
    }
    if let Some(coverage) = options.coverage {
        draft = draft.with_coverage(coverage);
    }
    // IMPORTANT: from `ingested`, the same buffer the digests above came from. Taken from the
    // pre-mark source instead, the reference would describe audio that exists nowhere on disk.
    let reference = genotrace::reference_fingerprint(ingested.audio())?;
    if let Some(fingerprint) = &reference {
        draft = draft.with_fingerprint(fingerprint.clone());
    }
    for claim in &options.claims {
        draft = draft.with_claim(claim.clone());
    }

    let unsigned = draft.to_unsigned_value()?;
    let signature = options
        .signer
        .sign_manifest(&unsigned, &options.public_key_file)?;
    let manifest_bytes = draft.seal(&signature)?;
    let record_id = RecordId::from_manifest_bytes(&manifest_bytes);

    let embedded_in = if options.embed_manifest {
        write_asset_with_record(&ingested, asset, &manifest_bytes)?;
        Some(asset.display().to_string())
    } else {
        if let Some(out) = &options.out {
            write_file(out, ingested.bytes())?;
        }
        None
    };

    let sidecar_path = match &options.sidecar {
        SidecarOutput::None => None,
        SidecarOutput::Explicit(target) => {
            write_file(target, &manifest_bytes)?;
            Some(target.display().to_string())
        }
        SidecarOutput::Conventional => {
            let target = genotrace::sidecar_paths(asset)
                .into_iter()
                .next()
                .ok_or_else(|| SdkError::InvalidOption {
                    option: "sidecar",
                    reason: "the asset path has no conventional sidecar".to_string(),
                })?;
            write_file(&target, &manifest_bytes)?;
            Some(target.display().to_string())
        }
    };

    Ok(SignResult {
        manifest_bytes,
        record_id: record_id.to_hex(),
        locator: hex::encode(derive_locator(
            &options.signer.public_key_bytes(),
            &options.locator_salt,
        )),
        locator_salt: options.locator_salt.to_hex(),
        signer_id: signature.signer_id.clone(),
        public_key_hex: signature.public_key_hex.clone(),
        content_sha256: ingested.content_sha256().to_string(),
        content_bytes: ingested.content_bytes(),
        decoded_audio_sha256,
        signed_at: options.signed_at.as_str().to_string(),
        signed_date,
        mark: options.mark,
        reference_fingerprint_hex_len: reference
            .as_ref()
            .map(|fingerprint| fingerprint.digest_hex().len()),
        sidecar_path,
        embedded_in,
        asset_path: asset.display().to_string(),
        asset_binds_content_sha256: !options.embed_manifest,
    })
}

/// Phase one of a publish: the locator, allocated before any audio is touched.
///
/// The salt is what the manifest will carry and the locator is what the mark will carry, so holding
/// both in one value is what stops the two halves from being allocated separately and disagreeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkPlan {
    salt: LocatorSalt,
    payload: Payload,
}

impl MarkPlan {
    pub fn allocate(
        signer: &(impl ManifestSigner + ?Sized),
        namespace: u8,
    ) -> Result<Self, SdkError> {
        Self::from_salt(signer, namespace, fresh_salt()?)
    }

    /// The same allocation from a caller-supplied salt: a deterministic test vector, or a locator
    /// this signer already published and is re-marking under.
    pub fn from_salt(
        signer: &(impl ManifestSigner + ?Sized),
        namespace: u8,
        salt: LocatorSalt,
    ) -> Result<Self, SdkError> {
        let locator = derive_locator(&signer.public_key_bytes(), &salt);
        Ok(Self {
            salt,
            payload: Payload::with_locator_bytes(genomark::payload::VERSION, namespace, &locator)?,
        })
    }

    pub const fn payload(&self) -> Payload {
        self.payload
    }

    pub const fn salt(&self) -> LocatorSalt {
        self.salt
    }

    pub fn locator_hex(&self) -> String {
        hex::encode(self.payload.locator_bytes())
    }
}

fn fresh_salt() -> Result<LocatorSalt, SdkError> {
    let mut raw = [0u8; LOCATOR_SALT_BYTES];
    getrandom::fill(&mut raw).map_err(|_| SdkError::Entropy {
        bytes: LOCATOR_SALT_BYTES,
    })?;
    Ok(LocatorSalt::from_bytes(raw))
}

#[derive(Debug)]
pub struct EmbedOptions {
    genomark: GenoMark,
    payload: Payload,
    out: PathBuf,
    depth: BitDepth,
    limits: IngestLimits,
}

impl EmbedOptions {
    /// Defaults to the public namespace, whose profile key is published: the public mark is
    /// provenance RECOVERY for cooperative and accidental cases, not tamper resistance.
    pub fn new(out: impl Into<PathBuf>, payload: Payload) -> Self {
        Self {
            genomark: GenoMark::public(),
            payload,
            out: out.into(),
            depth: BitDepth::Int24,
            limits: IngestLimits::default(),
        }
    }

    #[must_use]
    pub fn with_genomark(mut self, genomark: GenoMark) -> Self {
        self.genomark = genomark;
        self
    }

    #[must_use]
    pub const fn depth(mut self, depth: BitDepth) -> Self {
        self.depth = depth;
        self
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: IngestLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EmbedResult {
    pub algorithm: &'static str,
    pub out: String,
    pub payload_hex: String,
    pub locator_hex: String,
    pub version: u8,
    pub namespace: u8,
    pub payload_bits: usize,
    pub slots: usize,
    pub blocks: usize,
    pub punctured_slots: usize,
    pub passes: usize,
    pub max_applied_shift_nepers: f64,
    pub max_intended_shift_nepers: f64,
    pub closure_residual_nepers: Option<f64>,
    pub closure_residual_max_nepers: Option<f64>,
    pub measured_closure_gain: f64,
    /// False when the two-pass overlap-add closure did not settle under its budget. The mark is
    /// still written; the caller is told the residual is out of specification rather than not told.
    pub meets_closure_budget: bool,
    pub bin_gain_median_db: Option<f64>,
    pub bin_gain_p95_db: Option<f64>,
    pub bin_gain_max_db: Option<f64>,
    pub bin_gain_energy_weighted_rms_db: Option<f64>,
}

/// Writes a marked copy of an asset.
pub fn embed(path: &Path, options: &EmbedOptions) -> Result<EmbedResult, SdkError> {
    let ingested = ingest_path(path, options.limits)?;
    if carries_record(&ingested, path) {
        return Err(SdkError::MarkAfterSign {
            path: path.display().to_string(),
        });
    }

    let payload = options.payload;
    if payload.namespace() != options.genomark.namespace() {
        return Err(SdkError::InvalidOption {
            option: "payload",
            reason: format!(
                "the payload names namespace {}, which this detector profile is not keyed for",
                payload.namespace()
            ),
        });
    }

    let (marked, report) = options.genomark.embed_measured(ingested.audio(), payload)?;
    let encoded = audio_provenance_audio::wav::encode(&marked, options.depth)?;
    write_file(&options.out, &encoded)?;

    Ok(EmbedResult {
        algorithm: genomark::ALGORITHM_ID,
        out: options.out.display().to_string(),
        payload_hex: hex::encode(payload.to_bytes()),
        locator_hex: hex::encode(payload.locator_bytes()),
        version: payload.version(),
        namespace: payload.namespace(),
        payload_bits: genomark::PAYLOAD_BITS,
        slots: report.slots,
        blocks: report.blocks,
        punctured_slots: report.punctured_slots,
        passes: report.passes,
        max_applied_shift_nepers: report.max_applied_shift_nepers,
        max_intended_shift_nepers: report.max_intended_shift_nepers,
        closure_residual_nepers: report.closure_residual_nepers,
        closure_residual_max_nepers: report.closure_residual_max_nepers,
        measured_closure_gain: report.measured_closure_gain,
        meets_closure_budget: report.meets_closure_budget(),
        bin_gain_median_db: report.bin_gain.map(|gain| gain.median_db),
        bin_gain_p95_db: report.bin_gain.map(|gain| gain.p95_db),
        bin_gain_max_db: report.bin_gain.map(|gain| gain.max_db),
        bin_gain_energy_weighted_rms_db: report.bin_gain.map(|gain| gain.energy_weighted_rms_db),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishReceipt {
    pub registry: String,
    pub location: String,
    pub record_id: String,
    pub mark_id: String,
    pub locator: String,
    pub content_sha256: String,
    pub signed_at: String,
}

/// Stores a signed record in a writable local or authenticated remote registry.
///
/// The mark id is an INDEX KEY derived from the manifest's own key and salt. It is present whether
/// or not the asset carries a mark, and a record's presence under it is never itself evidence that
/// the audio was marked.
pub fn publish(
    registry: &dyn WritableRegistryBackend,
    result: &SignResult,
) -> Result<PublishReceipt, SdkError> {
    let manifest = parse_signing_input(&result.manifest_bytes)
        .map_err(audio_provenance_manifest::ManifestError::from)?;
    let record = RegistryRecord::from_signed_manifest(
        manifest,
        ContentHash::parse_hex(&result.content_sha256)?,
        None,
        SignedAt::parse(&result.signed_at)?,
    )?;

    let derived = record.record_id().to_hex();
    if derived != result.record_id {
        return Err(SdkError::RecordDigestMismatch {
            declared: result.record_id.clone(),
            derived,
        });
    }

    WritableRegistryBackend::put(registry, &record)?;
    Ok(PublishReceipt {
        registry: registry.source().name().to_string(),
        location: registry.source().location().to_string(),
        record_id: derived,
        mark_id: record.mark_id().to_hex(),
        locator: record.mark_id().locator_hex(),
        content_sha256: result.content_sha256.clone(),
        signed_at: result.signed_at.clone(),
    })
}

/// Opens a configured registry NAME for writing.
///
/// This compatibility helper intentionally returns only a filesystem backend. Production callers
/// construct an authenticated [`audio_provenance_registry::HttpRegistryBackend`] and pass it to
/// [`publish`] through [`WritableRegistryBackend`].
pub fn open_local_registry(
    name: &str,
    config_path: Option<&Path>,
) -> Result<LocalRegistryBackend, SdkError> {
    let resolver = RegistryResolver::load(config_path, &ProcessEnv)?;
    let backend = resolver.resolve(name)?;
    let source = backend.source();
    match source.kind() {
        RegistryKind::Local => Ok(LocalRegistryBackend::open(
            name,
            Path::new(source.location()),
        )?),
        kind @ (RegistryKind::Http | RegistryKind::Memory) => Err(SdkError::PublishUnsupported {
            name: name.to_string(),
            kind: kind.as_str(),
        }),
    }
}

/// Whether the asset already commits to a provenance record.
///
/// An unparseable provenance slot counts: a caller who put something there meant to, and re-marking
/// would destroy a binding they could still repair.
///
/// A container this build does not scan (Ogg, MP4) does NOT count, and the gate is therefore not
/// total for those two. That is the same blind spot GenoTrace's rung 1 reports as `unsearched`, so
/// a record hidden there is one no verifier in this build could recover either; the conventional
/// sidecar is the only check that still applies. A total gate would have to refuse every Ogg and
/// MP4 input outright, which would block marking unsigned audio on the strength of a scan that was
/// never written.
fn carries_record(ingested: &Ingested, path: &Path) -> bool {
    match find_embedded(ingested.bytes(), ingested.container()) {
        EmbeddedOutcome::Found(_) | EmbeddedOutcome::Unparseable(_) => return true,
        EmbeddedOutcome::Absent | EmbeddedOutcome::Unsearched(_) => {}
    }
    genotrace::sidecar_paths(path)
        .iter()
        .any(|candidate| candidate.is_file())
}

fn write_asset_with_record(
    ingested: &Ingested,
    asset: &Path,
    manifest_bytes: &[u8],
) -> Result<(), SdkError> {
    if ingested.container() != genotrace::ingest::Container::Wav {
        return Err(SdkError::ContainerNotWritable {
            container: ingested.container().as_str(),
        });
    }
    let bytes = riff::with_manifest_chunk(
        ingested.bytes(),
        manifest_bytes,
        &asset.display().to_string(),
    )?;
    write_file(asset, &bytes)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), SdkError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| SdkError::io(parent, source))?;
    }
    std::fs::write(path, bytes).map_err(|source| SdkError::io(path, source))
}
