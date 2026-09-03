#![cfg(feature = "http")]
#![allow(clippy::panic, clippy::unwrap_used)]

use audio_provenance_core::{ManifestSigner, SigningKey, canonical_json, sha256};
use audio_provenance_sdk::{CloudflareAuth, CloudflareSigner, SignerError};
use ed25519_dalek::Signer;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn key() -> SigningKey {
    SigningKey::from_raw_bytes(&[7; 32]).unwrap()
}

#[tokio::test]
async fn successful_signing_sends_auth_and_returns_decoded_bytes() {
    let server = MockServer::start().await;
    let digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let signature = "a1".repeat(64);
    Mock::given(method("POST"))
        .and(path("/sign"))
        .and(header("Authorization", "Bearer test_secret_token"))
        .and(header("Content-Type", "application/json"))
        .and(body_json(json!({ "digest": digest })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "algorithm": "Ed25519-SHA256",
            "signature": signature,
        })))
        .mount(&server)
        .await;

    let signer =
        CloudflareSigner::new(server.uri(), "test_secret_token", &key().public_key_hex()).unwrap();
    assert_eq!(signer.sign_digest(digest).await.unwrap(), vec![0xa1; 64]);
}

#[tokio::test]
async fn access_service_token_uses_both_access_headers_without_debug_leakage() {
    let server = MockServer::start().await;
    let digest = "ab".repeat(32);
    Mock::given(method("POST"))
        .and(path("/sign"))
        .and(header("CF-Access-Client-Id", "client-id.example.access"))
        .and(header("CF-Access-Client-Secret", "super-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "algorithm": "Ed25519-SHA256",
            "signature": "00".repeat(64),
        })))
        .mount(&server)
        .await;
    let signer = CloudflareSigner::with_auth(
        server.uri(),
        CloudflareAuth::AccessServiceToken {
            client_id: "client-id.example.access".into(),
            client_secret: "super-secret".into(),
            worker_bearer_token: "worker-secret".into(),
        },
        &key().public_key_hex(),
    )
    .unwrap();
    let debug = format!("{signer:?}");
    assert!(!debug.contains("super-secret"));
    signer.sign_digest(&digest).await.unwrap();
}

#[tokio::test]
async fn remote_error_statuses_are_preserved() {
    for (status, body) in [(401, "Unauthorized"), (400, "invalid digest")] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let signer =
            CloudflareSigner::new(server.uri(), "invalid", &key().public_key_hex()).unwrap();
        let error = signer.sign_digest(&"ab".repeat(32)).await.unwrap_err();
        match error {
            SignerError::Remote {
                status: observed,
                body: observed_body,
            } => {
                assert_eq!(observed, status);
                assert_eq!(observed_body, body);
            }
            other => panic!("expected remote error, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn malformed_hex_and_wrong_algorithm_are_rejected() {
    for (algorithm, signature, expected_algorithm_error) in [
        ("Ed25519-SHA256", "not-valid-hex!", false),
        ("Ed25519", "00", true),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "algorithm": algorithm,
                "signature": signature,
            })))
            .mount(&server)
            .await;
        let signer = CloudflareSigner::new(server.uri(), "token", &key().public_key_hex()).unwrap();
        let error = signer.sign_digest(&"cd".repeat(32)).await.unwrap_err();
        assert_eq!(
            matches!(error, SignerError::Algorithm { .. }),
            expected_algorithm_error
        );
    }
}

#[tokio::test]
async fn manifest_signing_verifies_the_returned_digest_signature_locally() {
    let server = MockServer::start().await;
    let unsigned = json!({"claim": "remote key custody", "version": 1});
    let digest = sha256(&canonical_json(&unsigned).unwrap());
    let dalek = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let signature = hex::encode(dalek.sign(&digest).to_bytes());
    Mock::given(method("POST"))
        .and(path("/sign"))
        .and(body_json(json!({ "digest": hex::encode(digest) })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "algorithm": "Ed25519-SHA256",
            "signature": signature,
        })))
        .mount(&server)
        .await;

    let signer = CloudflareSigner::new(server.uri(), "token", &key().public_key_hex()).unwrap();
    let portable = signer
        .sign_manifest(&unsigned, "cloudflare:hsm-signer")
        .unwrap();
    audio_provenance_core::verify_manifest_signature(&unsigned, &portable, None).unwrap();
}

#[test]
fn production_urls_require_tls_and_credentials_are_nonempty() {
    assert!(matches!(
        CloudflareSigner::new("http://signer.example", "token", &key().public_key_hex()),
        Err(SignerError::InvalidUrl(_))
    ));
    assert!(matches!(
        CloudflareSigner::new("https://signer.example", "", &key().public_key_hex()),
        Err(SignerError::InvalidCredential)
    ));
}
