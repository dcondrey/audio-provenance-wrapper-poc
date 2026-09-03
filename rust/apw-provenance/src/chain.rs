use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use p256::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DistinguishedName, DnType, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, SerialNumber,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::ParsedExtension;
use x509_parser::oid_registry::{
    OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256, OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER,
    OID_X509_EXT_SUBJECT_KEY_IDENTIFIER,
};
use x509_parser::pem::Pem;
use zeroize::Zeroizing;

use crate::error::{ProvenanceError, Result};

pub const ROOT_COMMON_NAME: &str = "Audio Provenance Wrapper Local Root CA";
pub const ORGANIZATION_NAME: &str = "Audio Provenance Wrapper";
pub const DEFAULT_CREATOR_COMMON_NAME: &str = "Audio Provenance Wrapper Local Creator";
pub const CERT_VALIDITY_DAYS: i64 = 365;
pub const ROOT_VALIDITY_DAYS: i64 = 3650;
pub const SIGNING_ALGORITHM: &str = "es256";

pub const CHAIN_VALID_REASON: &str = "chain validates against the configured trust anchor";

// REQUIRED: rcgen writes extendedKeyUsage non-critical, and c2pa-rs was measured
// against a leaf whose emailProtection EKU is critical. DER for
// `SEQUENCE { OID 1.3.6.1.5.5.7.3.4 }`, written as a critical custom extension
// with `extended_key_usages` left empty so the extension is not emitted twice.
const EKU_EMAIL_PROTECTION_DER: [u8; 12] =
    [0x30, 0x0a, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x04];
const OID_EXTENDED_KEY_USAGE: [u64; 4] = [2, 5, 29, 37];

/// The outcome of validating a leaf-then-issuer chain against a trust anchor.
///
/// IMPORTANT: `valid` means the chain is well-formed, in date, and signed by the
/// anchor. It says nothing about whether the anchor attests to a real identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainValidation {
    pub valid: bool,
    pub reason: String,
}

pub fn generate_key() -> SecretKey {
    SecretKey::random(&mut OsRng)
}

pub fn random_bytes(length: usize) -> Zeroizing<Vec<u8>> {
    let mut buffer = Zeroizing::new(vec![0u8; length]);
    OsRng.fill_bytes(buffer.as_mut_slice());
    buffer
}

pub fn key_to_pkcs8_pem(key: &SecretKey) -> Result<Zeroizing<String>> {
    key.to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| ProvenanceError::KeyMaterial(error.to_string()))
}

pub fn key_from_pkcs8_pem(pem: &str) -> Result<SecretKey> {
    SecretKey::from_pkcs8_pem(pem)
        .map_err(|error| ProvenanceError::KeyMaterial(error.to_string()))
}

/// Stable short identifier for a key: the first 16 hex characters of the SHA-256
/// of its DER SubjectPublicKeyInfo.
pub fn key_id_of(public_key: &PublicKey) -> Result<String> {
    let der = public_key
        .to_public_key_der()
        .map_err(|error| ProvenanceError::KeyMaterial(error.to_string()))?;
    let digest = Sha256::digest(der.as_bytes());
    Ok(hex_lower(&digest)
        .get(..16)
        .unwrap_or_default()
        .to_string())
}

pub fn issue_root(key: &SecretKey) -> Result<String> {
    let key_pair = key_pair_of(key)?;
    let mut params = base_params(ROOT_COMMON_NAME, ROOT_VALIDITY_DAYS)?;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(1));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params.self_signed(&key_pair)?.pem())
}

pub fn issue_leaf(
    leaf_key: &SecretKey,
    common_name: &str,
    root_key: &SecretKey,
    root_cert_pem: &str,
) -> Result<String> {
    let leaf_pair = key_pair_of(leaf_key)?;
    let root_pair = key_pair_of(root_key)?;
    // IMPORTANT: the issuer is rebuilt from the stored root certificate, not from
    // freshly built params. Its distinguished name and key-identifier method must
    // byte-match the certificate on disk or the reissued leaf's issuer field and
    // authorityKeyIdentifier stop matching the root across a restart.
    let issuer = Issuer::from_ca_cert_pem(root_cert_pem, root_pair)?;

    let mut params = base_params(common_name, CERT_VALIDITY_DAYS)?;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.use_authority_key_identifier_extension = true;
    let mut eku = CustomExtension::from_oid_content(
        &OID_EXTENDED_KEY_USAGE,
        EKU_EMAIL_PROTECTION_DER.to_vec(),
    );
    eku.set_criticality(true);
    params.custom_extensions = vec![eku];

    Ok(params.signed_by(&leaf_pair, &issuer)?.pem())
}

fn key_pair_of(key: &SecretKey) -> Result<KeyPair> {
    let pem = key_to_pkcs8_pem(key)?;
    KeyPair::from_pem(&pem).map_err(ProvenanceError::from)
}

fn base_params(common_name: &str, validity_days: i64) -> Result<CertificateParams> {
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, common_name);
    distinguished_name.push(DnType::OrganizationName, ORGANIZATION_NAME);

    let now = OffsetDateTime::now_utc();
    // rcgen marks CertificateParams #[non_exhaustive], so it cannot be built with
    // a struct expression from outside the crate.
    #[allow(clippy::field_reassign_with_default)]
    let params = {
        let mut params = CertificateParams::default();
        params.distinguished_name = distinguished_name;
        params.not_before = now - Duration::minutes(5);
        params.not_after = now + Duration::days(validity_days);
        params.serial_number = Some(SerialNumber::from(random_bytes(16).to_vec()));
        params
    };
    Ok(params)
}

/// Validate a leaf-then-issuer chain against a trust anchor.
///
/// Enforces the profile the C2PA signer accepts: a CA:TRUE / keyCertSign root and
/// a CA:FALSE leaf with critical digitalSignature, critical emailProtection, and
/// an authorityKeyIdentifier. A single self-signed certificate is rejected.
///
/// IMPORTANT: the authorityKeyIdentifier is not decorative. c2pa-rs rejects an
/// otherwise well-formed leaf without it as "the certificate is invalid",
/// measured against c2pa-python 0.90.15.
pub fn validate_certificate_chain(chain_pem: &[u8], trust_anchor_pem: &[u8]) -> ChainValidation {
    match validate(chain_pem, trust_anchor_pem) {
        Ok(()) => ChainValidation {
            valid: true,
            reason: CHAIN_VALID_REASON.to_string(),
        },
        Err(reason) => ChainValidation {
            valid: false,
            reason,
        },
    }
}

fn validate(chain_pem: &[u8], trust_anchor_pem: &[u8]) -> core::result::Result<(), String> {
    let chain_der = parse_pem_certificates(chain_pem)?;
    let anchor_der = parse_pem_certificates(trust_anchor_pem)?;

    let chain = parse_certificates(&chain_der)?;
    let anchors = parse_certificates(&anchor_der)?;

    let Some(leaf) = chain.first() else {
        return Err("chain is empty".to_string());
    };
    if anchors.is_empty() {
        return Err("no trust anchor supplied".to_string());
    }
    if leaf.issuer().as_raw() == leaf.subject().as_raw() {
        return Err("the certificate was self-signed".to_string());
    }

    let basic = leaf
        .basic_constraints()
        .ok()
        .flatten()
        .ok_or_else(|| missing_extension("basicConstraints"))?;
    let usage = leaf
        .key_usage()
        .ok()
        .flatten()
        .ok_or_else(|| missing_extension("keyUsage"))?;
    let eku = leaf
        .extended_key_usage()
        .ok()
        .flatten()
        .ok_or_else(|| missing_extension("extendedKeyUsage"))?;
    let leaf_aki = authority_key_identifier(leaf)
        .ok_or_else(|| missing_extension("authorityKeyIdentifier"))?;

    if basic.value.ca {
        return Err("leaf asserts CA:TRUE".to_string());
    }
    if !usage.value.digital_signature() {
        return Err("leaf keyUsage does not permit digitalSignature".to_string());
    }
    if !eku.value.email_protection {
        return Err("leaf extendedKeyUsage does not include emailProtection".to_string());
    }

    let now = x509_parser::time::ASN1Time::now();
    for certificate in chain.iter().chain(anchors.iter()) {
        if !certificate.validity().is_valid_at(now) {
            return Err(format!(
                "certificate outside its validity window: {}",
                certificate.subject()
            ));
        }
    }

    let issuer = chain
        .iter()
        .skip(1)
        .chain(anchors.iter())
        .find(|candidate| candidate.subject().as_raw() == leaf.issuer().as_raw())
        .ok_or_else(|| "leaf issuer is not present in the chain or trust anchors".to_string())?;

    let anchored = anchors.iter().any(|anchor| {
        anchor.subject().as_raw() == issuer.subject().as_raw()
            && anchor.public_key().raw == issuer.public_key().raw
    });
    if !anchored {
        return Err("chain does not terminate at a configured trust anchor".to_string());
    }

    let issuer_is_ca = issuer
        .basic_constraints()
        .ok()
        .flatten()
        .is_some_and(|basic| basic.value.ca);
    let issuer_can_sign = issuer
        .key_usage()
        .ok()
        .flatten()
        .is_some_and(|usage| usage.value.key_cert_sign());
    if !issuer_is_ca || !issuer_can_sign {
        return Err("issuer is not a certificate-signing CA".to_string());
    }

    let issuer_ski = subject_key_identifier(issuer)
        .ok_or_else(|| "issuer is missing a subjectKeyIdentifier".to_string())?;
    if leaf_aki != issuer_ski {
        return Err("leaf authorityKeyIdentifier does not match its issuer".to_string());
    }

    verify_leaf_signature(leaf, issuer)
}

/// Verify the leaf's signature with ES256 only.
///
/// IMPORTANT: the algorithm is pinned rather than taken from the certificate. The
/// issued profile is ES256 over P-256, and honouring whatever the certificate
/// names is the shape an algorithm-confusion attack needs.
fn verify_leaf_signature(
    leaf: &X509Certificate<'_>,
    issuer: &X509Certificate<'_>,
) -> core::result::Result<(), String> {
    // RFC 5280 4.1.1.2: the outer signatureAlgorithm and the tbsCertificate's
    // signature field must agree. Both are pinned to ES256 here.
    if leaf.signature_algorithm.algorithm != OID_SIG_ECDSA_WITH_SHA256
        || leaf.tbs_certificate.signature.algorithm != OID_SIG_ECDSA_WITH_SHA256
    {
        return Err("leaf signature algorithm is not ecdsa-with-SHA256".to_string());
    }
    if issuer.public_key().algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY {
        return Err("issuer public key is not an elliptic-curve key".to_string());
    }

    let verifying_key = VerifyingKey::from_sec1_bytes(&issuer.public_key().subject_public_key.data)
        .map_err(|_| "issuer public key is not a valid P-256 point".to_string())?;
    let signature = Signature::from_der(&leaf.signature_value.data)
        .map_err(|_| "leaf signature is not a valid ECDSA signature".to_string())?;
    verifying_key
        .verify(leaf.tbs_certificate.as_ref(), &signature)
        .map_err(|_| "leaf signature does not verify against its issuer".to_string())
}

fn missing_extension(name: &str) -> String {
    format!("leaf is missing a required extension: {name}")
}

fn authority_key_identifier<'a>(certificate: &'a X509Certificate<'a>) -> Option<&'a [u8]> {
    match certificate
        .get_extension_unique(&OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER)
        .ok()??
        .parsed_extension()
    {
        ParsedExtension::AuthorityKeyIdentifier(aki) => aki.key_identifier.as_ref().map(|id| id.0),
        _ => None,
    }
}

fn subject_key_identifier<'a>(certificate: &'a X509Certificate<'a>) -> Option<&'a [u8]> {
    match certificate
        .get_extension_unique(&OID_X509_EXT_SUBJECT_KEY_IDENTIFIER)
        .ok()??
        .parsed_extension()
    {
        ParsedExtension::SubjectKeyIdentifier(id) => Some(id.0),
        _ => None,
    }
}

/// Maximum certificates accepted in one PEM blob. Bounds the work an untrusted
/// chain can cause before any of it is parsed.
const MAX_CHAIN_CERTIFICATES: usize = 8;

fn parse_pem_certificates(pem: &[u8]) -> core::result::Result<Vec<Vec<u8>>, String> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for entry in Pem::iter_from_buffer(pem) {
        let entry = entry.map_err(|error| format!("chain could not be parsed: {error}"))?;
        if entry.label != "CERTIFICATE" {
            continue;
        }
        if out.len() == MAX_CHAIN_CERTIFICATES {
            return Err(format!(
                "chain could not be parsed: more than {MAX_CHAIN_CERTIFICATES} certificates"
            ));
        }
        out.push(entry.contents);
    }
    Ok(out)
}

fn parse_certificates(der: &[Vec<u8>]) -> core::result::Result<Vec<X509Certificate<'_>>, String> {
    der.iter()
        .map(|bytes| {
            x509_parser::parse_x509_certificate(bytes)
                .map(|(_, certificate)| certificate)
                .map_err(|error| format!("chain could not be parsed: {error}"))
        })
        .collect()
}

pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(nibble(byte >> 4));
        out.push(nibble(byte & 0x0f));
    }
    out
}

fn nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + value - 10) as char,
    }
}

pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = (*pair.first()? as char).to_digit(16)?;
        let low = (*pair.get(1)? as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
    }
    Some(out)
}
