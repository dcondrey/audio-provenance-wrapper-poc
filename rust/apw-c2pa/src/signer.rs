use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use c2pa::{create_signer, Builder, Context, Signer, SigningAlg};
use serde_json::Value;

use crate::error::{file_name, io_error, C2paError, Result};
use crate::format::{detect_format, AssetFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningAlgorithm {
    Es256,
    Es384,
    Es512,
    Ps256,
    Ps384,
    Ps512,
    Ed25519,
}

impl SigningAlgorithm {
    pub const ALL: [SigningAlgorithm; 7] = [
        SigningAlgorithm::Es256,
        SigningAlgorithm::Es384,
        SigningAlgorithm::Es512,
        SigningAlgorithm::Ps256,
        SigningAlgorithm::Ps384,
        SigningAlgorithm::Ps512,
        SigningAlgorithm::Ed25519,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SigningAlgorithm::Es256 => "es256",
            SigningAlgorithm::Es384 => "es384",
            SigningAlgorithm::Es512 => "es512",
            SigningAlgorithm::Ps256 => "ps256",
            SigningAlgorithm::Ps384 => "ps384",
            SigningAlgorithm::Ps512 => "ps512",
            SigningAlgorithm::Ed25519 => "ed25519",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        let lowered = value.to_ascii_lowercase();
        SigningAlgorithm::ALL
            .into_iter()
            .find(|alg| alg.as_str() == lowered)
            .ok_or_else(|| {
                C2paError::SignerSetup(format!("Unsupported signing algorithm {value:?}"))
            })
    }

    fn to_c2pa(self) -> SigningAlg {
        match self {
            SigningAlgorithm::Es256 => SigningAlg::Es256,
            SigningAlgorithm::Es384 => SigningAlg::Es384,
            SigningAlgorithm::Es512 => SigningAlg::Es512,
            SigningAlgorithm::Ps256 => SigningAlg::Ps256,
            SigningAlgorithm::Ps384 => SigningAlg::Ps384,
            SigningAlgorithm::Ps512 => SigningAlg::Ps512,
            SigningAlgorithm::Ed25519 => SigningAlg::Ed25519,
        }
    }
}

/// A configured C2PA claim signer.
///
/// IMPORTANT: possession of this signer proves control of a private key. It is not, and
/// must never be presented as, a verified identity for whoever holds it.
pub struct C2paSigner {
    inner: c2pa::BoxedSigner,
}

impl core::fmt::Debug for C2paSigner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("C2paSigner").finish_non_exhaustive()
    }
}

impl C2paSigner {
    /// Build a signer from a leaf-then-root PEM chain and the leaf's PKCS#8 private key.
    ///
    /// REQUIRED: a lone self-signed certificate is rejected by C2PA validation
    /// ("the certificate was self-signed"), so the chain must carry the signing leaf
    /// followed by at least one CA certificate.
    pub fn from_pem(
        cert_chain_pem: &[u8],
        private_key_pem: &[u8],
        alg: SigningAlgorithm,
    ) -> Result<Self> {
        if count_certificates(cert_chain_pem) < 2 {
            return Err(C2paError::SignerSetup(
                "Certificate chain must contain the signing leaf followed by at least one CA \
                 certificate; a lone self-signed certificate is rejected by C2PA validation"
                    .to_string(),
            ));
        }
        if !contains(private_key_pem, b"PRIVATE KEY-----") {
            return Err(C2paError::SignerSetup(
                "Private key must be a PEM-encoded private key".to_string(),
            ));
        }
        let inner = create_signer::from_keys(cert_chain_pem, private_key_pem, alg.to_c2pa(), None)
            .map_err(|e| C2paError::SignerSetup(format!("Building the C2PA signer failed: {e}")))?;
        Ok(Self { inner })
    }

    fn as_dyn(&self) -> &dyn Signer {
        self.inner.as_ref()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn count_certificates(pem: &[u8]) -> usize {
    let marker = b"-----BEGIN CERTIFICATE-----";
    if pem.len() < marker.len() {
        return 0;
    }
    pem.windows(marker.len())
        .filter(|window| *window == marker)
        .count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningMode {
    Embedded,
    Sidecar,
}

impl SigningMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SigningMode::Embedded => "embedded",
            SigningMode::Sidecar => "sidecar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ByteRegion {
    pub start: u64,
    pub length: u64,
}

/// What the hard binding actually covers, recorded verbatim into the provenance record.
///
/// IMPORTANT: the sidecar binding has NO exclusions. It hashes every byte of the asset,
/// so any container rewrite, tag edit or re-mux breaks it. The embedded WAV binding
/// excludes only the appended manifest chunk, so every original audio byte is covered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardBinding {
    #[serde(rename = "type")]
    pub binding_type: String,
    pub algorithm: String,
    pub covers: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded_region: Option<ByteRegion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusions: Option<Vec<ByteRegion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewritten_header_field: Option<ByteRegion>,
    pub container_rewrite_tolerance: String,
    pub source_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl HardBinding {
    pub fn is_whole_file(&self) -> bool {
        self.exclusions.as_ref().is_some_and(Vec::is_empty) && self.excluded_region.is_none()
    }

    pub fn to_value(&self) -> Result<Value> {
        Ok(serde_json::to_value(self)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningResult {
    pub mode: SigningMode,
    pub asset_path: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub mime: String,
    pub binding: HardBinding,
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
    }
    Ok(())
}

fn builder_for(manifest: &Value) -> Result<Builder> {
    Builder::from_context(Context::new())
        .with_definition(manifest.clone())
        .map_err(|e| C2paError::Context(Box::new(e)))
}

/// Embed the manifest into a format c2pa-rs can carry it in (WAV today).
pub fn sign_embedded(
    src: &Path,
    dest: &Path,
    manifest: &Value,
    signer: &C2paSigner,
) -> Result<SigningResult> {
    let asset = detect_format(src)?;
    if !asset.embeddable {
        return Err(C2paError::UnsupportedAsset(format!(
            "{}: {} cannot carry an embedded manifest; use sign_sidecar",
            file_name(src),
            asset.container.as_str()
        )));
    }
    let original_length = std::fs::metadata(src).map_err(|e| io_error(src, e))?.len();
    ensure_parent(dest)?;

    let mut builder = builder_for(manifest)?;
    let mut source = File::open(src).map_err(|e| io_error(src, e))?;
    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .map_err(|e| io_error(dest, e))?;
    let signed = builder.sign(signer.as_dyn(), asset.mime, &mut source, &mut target);
    if let Err(source) = signed {
        // The destination was truncated on open; leaving a zero-length file where a signed
        // asset is expected is worse than leaving nothing.
        drop(target);
        let _ = std::fs::remove_file(dest);
        return Err(C2paError::Signing {
            mode: "Embedded",
            name: file_name(src),
            source: Box::new(source),
        });
    }
    let signed_length = target.metadata().map_err(|e| io_error(dest, e))?.len();

    Ok(SigningResult {
        mode: SigningMode::Embedded,
        asset_path: dest.to_path_buf(),
        manifest_path: Some(dest.to_path_buf()),
        mime: asset.mime.to_string(),
        binding: HardBinding {
            binding_type: "c2pa.hash.data".to_string(),
            algorithm: "sha256".to_string(),
            covers: "every original asset byte except the RIFF size field, which is rewritten \
                     to count the appended manifest chunk"
                .to_string(),
            excluded_region: Some(ByteRegion {
                start: original_length,
                length: signed_length.saturating_sub(original_length),
            }),
            exclusions: None,
            rewritten_header_field: Some(ByteRegion {
                start: 4,
                length: 4,
            }),
            container_rewrite_tolerance: "manifest chunk only".to_string(),
            source_sha256: apw_core::sha256_file(src)?,
            note: None,
        },
    })
}

/// Write a detached `.c2pa` manifest for an asset that cannot carry one.
///
/// IMPORTANT: the resulting binding is a whole-file hash with no exclusions.
pub fn sign_sidecar(
    src: &Path,
    dest_manifest_path: &Path,
    manifest: &Value,
    signer: &C2paSigner,
    mime: Option<&str>,
) -> Result<SigningResult> {
    let asset = detect_format(src)?;
    let mime = mime.unwrap_or(asset.mime).to_string();

    let mut builder = builder_for(manifest)?;
    builder.set_no_embed(true);
    let mut source = File::open(src).map_err(|e| io_error(src, e))?;
    // The sidecar path still makes c2pa-rs write a verbatim copy of the asset; it goes to a
    // temporary file rather than memory so a large export cannot be held in RAM.
    let mut sink = tempfile::tempfile().map_err(|e| io_error(src, e))?;
    let manifest_bytes = builder
        .sign(signer.as_dyn(), &mime, &mut source, &mut sink)
        .map_err(|source| C2paError::Signing {
            mode: "Sidecar",
            name: file_name(src),
            source: Box::new(source),
        })?;

    ensure_parent(dest_manifest_path)?;
    std::fs::write(dest_manifest_path, &manifest_bytes)
        .map_err(|e| io_error(dest_manifest_path, e))?;

    Ok(SigningResult {
        mode: SigningMode::Sidecar,
        asset_path: src.to_path_buf(),
        manifest_path: Some(dest_manifest_path.to_path_buf()),
        mime,
        binding: HardBinding {
            binding_type: "c2pa.hash.data".to_string(),
            algorithm: "sha256".to_string(),
            covers: "whole file".to_string(),
            excluded_region: None,
            exclusions: Some(Vec::new()),
            rewritten_header_field: None,
            container_rewrite_tolerance: "none".to_string(),
            source_sha256: apw_core::sha256_file(src)?,
            note: Some(
                "The sidecar binding hashes every byte of the asset with no exclusions, so \
                 any container rewrite, tag edit or re-mux breaks it."
                    .to_string(),
            ),
        },
    })
}

/// Route an asset to the embedded or sidecar path based on what the file actually is.
pub fn sign_asset(
    src: &Path,
    dest: &Path,
    manifest: &Value,
    signer: &C2paSigner,
    sidecar_path: Option<&Path>,
) -> Result<SigningResult> {
    let asset: AssetFormat = detect_format(src)?;
    if asset.embeddable {
        return sign_embedded(src, dest, manifest, signer);
    }
    let default_sidecar = dest.with_extension("c2pa");
    let sidecar = sidecar_path.unwrap_or(&default_sidecar);
    sign_sidecar(src, sidecar, manifest, signer, Some(asset.mime))
}
