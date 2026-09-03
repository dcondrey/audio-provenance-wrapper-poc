//! Authenticated client for the Cloudflare remote key-custody Worker.

use std::fmt;

use async_trait::async_trait;
use audio_provenance_core::{
    Canonicalization, ManifestSigner, PortableSignature, ProofLevel, SignatureAlgorithm,
    SignatureError, SignerIdentity, TrustScope, canonical_json, sha256, sha256_hex,
    signer_id_for_public_key,
};
use ed25519_dalek::{Signature, VerifyingKey};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const EXPECTED_ALGORITHM: &str = "Ed25519-SHA256";
const REMOTE_SIGNATURE_NOTES: &str = "The public key independently verifies canonical manifest integrity. The signing operation used authenticated remote key custody; a valid signature proves key possession, not creator identity, authorship, or external trust.";

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("Cloudflare signer URL is invalid: {0}")]
    InvalidUrl(String),
    #[error("Cloudflare signer credential cannot be represented as an HTTP header")]
    InvalidCredential,
    #[error("digest must be exactly 64 lowercase hexadecimal characters")]
    InvalidDigest,
    #[error("HTTP transport error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Cloudflare Worker returned error status {status}: {body}")]
    Remote { status: u16, body: String },
    #[error("Cloudflare Worker response exceeded {limit} bytes")]
    ResponseTooLarge { limit: usize },
    #[error("Cloudflare Worker returned algorithm {found:?}, expected {EXPECTED_ALGORITHM}")]
    Algorithm { found: String },
    #[error("failed to decode hex signature from remote signer: {0}")]
    HexDecode(#[from] hex::FromHexError),
    #[error("remote Ed25519 signature must contain 64 bytes, found {found}")]
    SignatureLength { found: usize },
    #[error("configured Ed25519 public key must contain 32 bytes, found {found}")]
    PublicKeyLength { found: usize },
    #[error(
        "remote signer returned a signature that does not verify under its configured public key"
    )]
    SignatureMismatch,
    #[error("could not start the synchronous remote-signing runtime: {0}")]
    Runtime(String),
    #[error("the synchronous remote-signing worker thread panicked")]
    WorkerPanicked,
}

/// Authentication attached to every signing request. Secret values are redacted from `Debug`.
#[derive(Clone)]
pub enum CloudflareAuth {
    Bearer(String),
    AccessServiceToken {
        client_id: String,
        client_secret: String,
        worker_bearer_token: String,
    },
}

impl fmt::Debug for CloudflareAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer(_) => f.write_str("Bearer([REDACTED])"),
            Self::AccessServiceToken { .. } => {
                f.write_str("AccessServiceToken { client_id: [REDACTED], client_secret: [REDACTED], worker_bearer_token: [REDACTED] }")
            }
        }
    }
}

impl CloudflareAuth {
    fn headers(&self) -> Result<HeaderMap, SignerError> {
        let mut headers = HeaderMap::new();
        let insert_sensitive =
            |headers: &mut HeaderMap, name: HeaderName, raw: &str| -> Result<(), SignerError> {
                let mut value =
                    HeaderValue::from_str(raw).map_err(|_| SignerError::InvalidCredential)?;
                value.set_sensitive(true);
                headers.insert(name, value);
                Ok(())
            };
        match self {
            Self::Bearer(token) => {
                require_credential(token)?;
                insert_sensitive(&mut headers, AUTHORIZATION, &format!("Bearer {token}"))?;
            }
            Self::AccessServiceToken {
                client_id,
                client_secret,
                worker_bearer_token,
            } => {
                require_credential(client_id)?;
                require_credential(client_secret)?;
                require_credential(worker_bearer_token)?;
                insert_sensitive(
                    &mut headers,
                    AUTHORIZATION,
                    &format!("Bearer {worker_bearer_token}"),
                )?;
                insert_sensitive(
                    &mut headers,
                    HeaderName::from_static("cf-access-client-id"),
                    client_id,
                )?;
                insert_sensitive(
                    &mut headers,
                    HeaderName::from_static("cf-access-client-secret"),
                    client_secret,
                )?;
            }
        }
        Ok(headers)
    }
}

#[derive(Serialize)]
struct RemoteSignRequest<'a> {
    digest: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteSignResponse {
    algorithm: String,
    signature: String,
}

#[async_trait]
pub trait RemoteSigner: Send + Sync {
    async fn sign(&self, digest_hex: &str) -> Result<Vec<u8>, SignerError>;
}

/// Remote Cloudflare Worker signer, pinned to the public key its signatures must verify under.
pub struct CloudflareSigner {
    endpoint: reqwest::Url,
    auth: CloudflareAuth,
    public_key: [u8; 32],
    client: reqwest::Client,
}

impl fmt::Debug for CloudflareSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudflareSigner")
            .field("endpoint", &self.endpoint)
            .field("auth", &self.auth)
            .field("signer_id", &signer_id_for_public_key(&self.public_key))
            .finish()
    }
}

impl CloudflareSigner {
    pub fn new(
        worker_url: impl AsRef<str>,
        bearer_token: impl Into<String>,
        public_key_hex: &str,
    ) -> Result<Self, SignerError> {
        Self::with_auth(
            worker_url,
            CloudflareAuth::Bearer(bearer_token.into()),
            public_key_hex,
        )
    }

    pub fn with_auth(
        worker_url: impl AsRef<str>,
        auth: CloudflareAuth,
        public_key_hex: &str,
    ) -> Result<Self, SignerError> {
        let mut endpoint = reqwest::Url::parse(worker_url.as_ref())
            .map_err(|error| SignerError::InvalidUrl(error.to_string()))?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(SignerError::InvalidUrl(
                "must be an absolute HTTP(S) URL without credentials, query, or fragment".into(),
            ));
        }
        if endpoint.scheme() == "http"
            && !matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        {
            return Err(SignerError::InvalidUrl(
                "plain HTTP is permitted only for a loopback test endpoint".into(),
            ));
        }
        endpoint.set_path(&format!("{}/sign", endpoint.path().trim_end_matches('/')));
        let public_key: [u8; 32] = hex::decode(public_key_hex)
            .map_err(SignerError::HexDecode)?
            .try_into()
            .map_err(|raw: Vec<u8>| SignerError::PublicKeyLength { found: raw.len() })?;
        VerifyingKey::from_bytes(&public_key).map_err(|_| SignerError::SignatureMismatch)?;
        let headers = auth.headers()?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .build()?;
        Ok(Self {
            endpoint,
            auth,
            public_key,
            client,
        })
    }

    pub async fn sign_digest(&self, digest_hex: &str) -> Result<Vec<u8>, SignerError> {
        validate_digest(digest_hex)?;
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .json(&RemoteSignRequest { digest: digest_hex })
            .send()
            .await?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(SignerError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(SignerError::ResponseTooLarge {
                    limit: MAX_RESPONSE_BYTES,
                });
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(SignerError::Remote {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&body).into_owned(),
            });
        }
        let payload: RemoteSignResponse =
            serde_json::from_slice(&body).map_err(|error| SignerError::Remote {
                status: status.as_u16(),
                body: format!("malformed JSON response: {error}"),
            })?;
        if payload.algorithm != EXPECTED_ALGORITHM {
            return Err(SignerError::Algorithm {
                found: payload.algorithm,
            });
        }
        Ok(hex::decode(payload.signature)?)
    }

    pub fn sign_digest_blocking(&self, digest_hex: &str) -> Result<Vec<u8>, SignerError> {
        let run = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| SignerError::Runtime(error.to_string()))?
                .block_on(self.sign_digest(digest_hex))
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(run)
                    .join()
                    .map_err(|_| SignerError::WorkerPanicked)?
            })
        } else {
            run()
        }
    }

    fn verified_signature(&self, digest: &[u8; 32]) -> Result<[u8; 64], SignerError> {
        let raw = self.sign_digest_blocking(&hex::encode(digest))?;
        let signature: [u8; 64] = raw
            .try_into()
            .map_err(|raw: Vec<u8>| SignerError::SignatureLength { found: raw.len() })?;
        VerifyingKey::from_bytes(&self.public_key)
            .map_err(|_| SignerError::SignatureMismatch)?
            .verify_strict(digest, &Signature::from_bytes(&signature))
            .map_err(|_| SignerError::SignatureMismatch)?;
        Ok(signature)
    }
}

#[async_trait]
impl RemoteSigner for CloudflareSigner {
    async fn sign(&self, digest_hex: &str) -> Result<Vec<u8>, SignerError> {
        self.sign_digest(digest_hex).await
    }
}

impl ManifestSigner for CloudflareSigner {
    fn public_key_bytes(&self) -> [u8; 32] {
        self.public_key
    }

    fn sign_manifest(
        &self,
        unsigned_manifest: &serde_json::Value,
        public_key_file: &str,
    ) -> Result<PortableSignature, SignatureError> {
        let content = canonical_json(unsigned_manifest)?;
        let digest = sha256(&content);
        let signature =
            self.verified_signature(&digest)
                .map_err(|error| SignatureError::RemoteSigning {
                    detail: error.to_string(),
                })?;
        Ok(PortableSignature {
            algorithm: SignatureAlgorithm::Ed25519Sha256,
            canonicalization: Canonicalization::ApwJsonSortV1,
            public_key_hex: hex::encode(self.public_key),
            public_key_file: public_key_file.to_owned(),
            signer_id: signer_id_for_public_key(&self.public_key),
            signature_hex: hex::encode(signature),
            signed_content_hash: sha256_hex(&content),
            trust_scope: TrustScope::RemoteKeyCustodyIntegrity,
            signer_identity: SignerIdentity::NotEstablished,
            signer_identity_proof_level: ProofLevel::UnknownUnobserved,
            proof_level: ProofLevel::DirectlyObserved,
            notes: REMOTE_SIGNATURE_NOTES.to_owned(),
        })
    }
}

fn validate_digest(digest: &str) -> Result<(), SignerError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(SignerError::InvalidDigest)
    }
}

fn require_credential(value: &str) -> Result<(), SignerError> {
    if !value.is_empty() && value.len() <= 4096 {
        Ok(())
    } else {
        Err(SignerError::InvalidCredential)
    }
}
