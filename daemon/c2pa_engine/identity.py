from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Iterable, Mapping, Sequence

import cbor2
from cryptography import x509
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import (
    decode_dss_signature,
    encode_dss_signature,
)

IDENTITY_ASSERTION_LABEL = "cawg.identity"

SIG_TYPE_X509_COSE = "cawg.x509.cose"
SIG_TYPE_ICA = "cawg.identity_claims_aggregation"
SIG_TYPES = frozenset({SIG_TYPE_X509_COSE, SIG_TYPE_ICA})

ROLES = frozenset(
    {
        "cawg.creator",
        "cawg.contributor",
        "cawg.editor",
        "cawg.producer",
        "cawg.publisher",
        "cawg.sponsor",
        "cawg.translator",
    }
)

HARD_BINDING_LABELS = frozenset({"c2pa.hash.data", "c2pa.hash.bmff.v2", "c2pa.hash.boxes"})

_COSE_SIGN1_TAG = 18
_COSE_ALG_ES256 = -7
_COSE_HEADER_ALG = 1
_COSE_HEADER_X5CHAIN = 33


class IdentityAssertionError(ValueError):
    pass


class IdentityProviderRequired(RuntimeError):
    pass


def _encode_head(major: int, argument: int) -> bytes:
    if argument < 24:
        return bytes([(major << 5) | argument])
    for extra, threshold in ((1, 0x100), (2, 0x10000), (4, 0x100000000), (8, 1 << 64)):
        if argument < threshold:
            info = {1: 24, 2: 25, 4: 26, 8: 27}[extra]
            return bytes([(major << 5) | info]) + argument.to_bytes(extra, "big")
    raise IdentityAssertionError("Integer too large for deterministic CBOR encoding")


def deterministic_cbor(value: object) -> bytes:
    """RFC 8949 section 4.2.1 core deterministic encoding.

    IMPORTANT: keys are sorted bytewise over their encoded form, so the ordering is not
    the source ordering and not a plain sort of the key strings.
    """
    if isinstance(value, bool):
        return cbor2.dumps(value)
    if isinstance(value, int):
        if value >= 0:
            return _encode_head(0, value)
        return _encode_head(1, -value - 1)
    if isinstance(value, (bytes, bytearray)):
        data = bytes(value)
        return _encode_head(2, len(data)) + data
    if isinstance(value, str):
        data = value.encode("utf-8")
        return _encode_head(3, len(data)) + data
    if isinstance(value, (list, tuple)):
        return _encode_head(4, len(value)) + b"".join(deterministic_cbor(item) for item in value)
    if isinstance(value, Mapping):
        encoded = [(deterministic_cbor(k), deterministic_cbor(v)) for k, v in value.items()]
        encoded.sort(key=lambda pair: pair[0])
        return _encode_head(5, len(encoded)) + b"".join(k + v for k, v in encoded)
    if value is None:
        return b"\xf6"
    raise IdentityAssertionError(f"Unsupported CBOR type: {type(value).__name__}")


@dataclass(frozen=True)
class HashedUri:
    url: str
    hash: bytes
    alg: str = "sha256"

    def to_map(self) -> dict[str, object]:
        if not self.url:
            raise IdentityAssertionError("A hashed URI requires a non-empty url")
        if not self.hash:
            raise IdentityAssertionError(f"Hashed URI {self.url} has an empty hash")
        return {"url": self.url, "hash": bytes(self.hash), "alg": self.alg}

    @property
    def label(self) -> str:
        return self.url.rsplit("/", 1)[-1].split("__", 1)[0]


def _validate_referenced_assertions(referenced: Sequence[HashedUri]) -> None:
    if not referenced:
        raise IdentityAssertionError("referenced_assertions MUST contain at least one entry")
    urls = [ref.url for ref in referenced]
    if len(set(urls)) != len(urls):
        raise IdentityAssertionError("referenced_assertions MUST NOT reference an assertion twice")
    hard_bindings = [ref for ref in referenced if ref.label in HARD_BINDING_LABELS]
    if not hard_bindings:
        raise IdentityAssertionError(
            "referenced_assertions MUST include the hard binding assertion "
            f"(one of {sorted(HARD_BINDING_LABELS)})"
        )
    if len(hard_bindings) > 1:
        raise IdentityAssertionError("referenced_assertions MUST NOT include a second hard binding")
    if any(ref.label == IDENTITY_ASSERTION_LABEL for ref in referenced):
        raise IdentityAssertionError("referenced_assertions MUST NOT reference the identity assertion itself")


def build_signer_payload(
    referenced_assertions: Sequence[HashedUri],
    sig_type: str,
    roles: Iterable[str] | None = None,
) -> dict[str, object]:
    if sig_type not in SIG_TYPES:
        raise IdentityAssertionError(f"sig_type MUST be one of {sorted(SIG_TYPES)}")
    _validate_referenced_assertions(referenced_assertions)
    payload: dict[str, object] = {
        "referenced_assertions": [ref.to_map() for ref in referenced_assertions],
        "sig_type": sig_type,
    }
    if roles is not None:
        role_list = list(roles)
        unknown = [role for role in role_list if role not in ROLES]
        if unknown:
            raise IdentityAssertionError(f"Unknown CAWG role(s): {unknown}")
        if not role_list:
            raise IdentityAssertionError("role MUST contain at least one entry when present")
        payload["role"] = role_list
    return payload


def encode_signer_payload(signer_payload: Mapping[str, object]) -> bytes:
    return deterministic_cbor(signer_payload)


def _der_certificates(cert_chain_pem: bytes) -> list[bytes]:
    certs = x509.load_pem_x509_certificates(cert_chain_pem)
    if not certs:
        raise IdentityAssertionError("Certificate chain contains no certificates")
    return [cert.public_bytes(serialization.Encoding.DER) for cert in certs]


def _sig_structure(protected: bytes, payload: bytes) -> bytes:
    return deterministic_cbor(["Signature1", protected, b"", payload])


def _p1363(signature_der: bytes, key_size_bytes: int) -> bytes:
    r, s = decode_dss_signature(signature_der)
    return r.to_bytes(key_size_bytes, "big") + s.to_bytes(key_size_bytes, "big")


def sign_signer_payload_x509(
    signer_payload: Mapping[str, object],
    cert_chain_pem: bytes,
    private_key_pem: bytes,
) -> bytes:
    """COSE_Sign1 (tag 18, detached payload) over the deterministic CBOR signer_payload."""
    if signer_payload.get("sig_type") != SIG_TYPE_X509_COSE:
        raise IdentityAssertionError(
            f"sign_signer_payload_x509 requires sig_type {SIG_TYPE_X509_COSE}"
        )
    key = serialization.load_pem_private_key(private_key_pem, password=None)
    if not isinstance(key, ec.EllipticCurvePrivateKey) or key.curve.name != "secp256r1":
        raise IdentityAssertionError("The x509 COSE path implements ES256 (P-256) only")
    der_certs = _der_certificates(cert_chain_pem)
    x5chain: object = der_certs[0] if len(der_certs) == 1 else der_certs
    protected = deterministic_cbor({_COSE_HEADER_ALG: _COSE_ALG_ES256, _COSE_HEADER_X5CHAIN: x5chain})
    payload = encode_signer_payload(signer_payload)
    signature = _p1363(key.sign(_sig_structure(protected, payload), ec.ECDSA(hashes.SHA256())), 32)
    return cbor2.dumps(cbor2.CBORTag(_COSE_SIGN1_TAG, [protected, {}, None, signature]))


def verify_signer_payload_x509(signature: bytes, signer_payload: Mapping[str, object]) -> x509.Certificate:
    decoded = cbor2.loads(signature)
    if isinstance(decoded, cbor2.CBORTag):
        if decoded.tag != _COSE_SIGN1_TAG:
            raise IdentityAssertionError(f"Expected COSE_Sign1 tag 18, found tag {decoded.tag}")
        decoded = decoded.value
    if not isinstance(decoded, (list, tuple)) or len(decoded) != 4:
        raise IdentityAssertionError("Malformed COSE_Sign1 structure")
    protected, _unprotected, embedded_payload, raw_signature = decoded
    if embedded_payload is not None:
        raise IdentityAssertionError("The identity assertion COSE payload MUST be detached")
    headers = cbor2.loads(protected)
    if headers.get(_COSE_HEADER_ALG) != _COSE_ALG_ES256:
        raise IdentityAssertionError("Only ES256 identity signatures are verified by this engine")
    chain = headers.get(_COSE_HEADER_X5CHAIN)
    der_leaf = chain[0] if isinstance(chain, list) else chain
    if not isinstance(der_leaf, (bytes, bytearray)):
        raise IdentityAssertionError("COSE x5chain header is missing or malformed")
    leaf = x509.load_der_x509_certificate(bytes(der_leaf))
    public_key = leaf.public_key()
    if not isinstance(public_key, ec.EllipticCurvePublicKey):
        raise IdentityAssertionError("Identity signing certificate does not carry an EC public key")
    if len(raw_signature) != 64:
        raise IdentityAssertionError("ES256 signature MUST be 64 bytes in P1363 form")
    der = encode_dss_signature(
        int.from_bytes(raw_signature[:32], "big"), int.from_bytes(raw_signature[32:], "big")
    )
    tbs = _sig_structure(bytes(protected), encode_signer_payload(signer_payload))
    try:
        public_key.verify(der, tbs, ec.ECDSA(hashes.SHA256()))
    except InvalidSignature as exc:
        raise IdentityAssertionError("cawg.x509.signature.mismatch") from exc
    return leaf


@dataclass(frozen=True)
class IdentityAssertion:
    signer_payload: dict[str, object]
    signature: bytes
    pad1: bytes
    pad2: bytes | None
    proof_level: str
    proof_note: str

    def to_map(self) -> dict[str, object]:
        assertion: dict[str, object] = {
            "signer_payload": self.signer_payload,
            "signature": self.signature,
            "pad1": self.pad1,
        }
        if self.pad2 is not None:
            assertion["pad2"] = self.pad2
        return assertion

    def to_cbor(self) -> bytes:
        return deterministic_cbor(self.to_map())


def _pad(length: int) -> bytes:
    if length < 0:
        raise IdentityAssertionError("Pad length cannot be negative")
    return b"\x00" * length


def build_x509_identity_assertion(
    referenced_assertions: Sequence[HashedUri],
    cert_chain_pem: bytes,
    private_key_pem: bytes,
    roles: Iterable[str] | None = None,
    pad1_length: int = 32,
) -> IdentityAssertion:
    """Sign a CAWG identity assertion with an X.509 key.

    IMPORTANT: this path is always user_declared. It used to take an
    identity_verified_by_provider boolean that promoted the assertion to
    externally_verified with no evidence that any external verification had
    occurred, which let any caller fabricate the strongest proof level in the
    vocabulary. externally_verified is reachable only through
    build_ica_identity_assertion, which requires a credential and a signature
    from the identity provider that issued it.
    """
    signer_payload = build_signer_payload(referenced_assertions, SIG_TYPE_X509_COSE, roles)
    signature = sign_signer_payload_x509(signer_payload, cert_chain_pem, private_key_pem)
    return IdentityAssertion(
        signer_payload=signer_payload,
        signature=signature,
        pad1=_pad(pad1_length),
        pad2=None,
        proof_level="user_declared",
        proof_note=(
            "This assertion proves possession of the signing key only. A self-generated or "
            "self-administered certificate carries no verified identity."
        ),
    )


@dataclass(frozen=True)
class VerifiedIdentity:
    type: str
    provider_id: str
    provider_name: str
    verified_at: str
    name: str | None = None
    username: str | None = None
    uri: str | None = None

    def to_map(self) -> dict[str, object]:
        entry: dict[str, object] = {
            "type": self.type,
            "provider": {"id": self.provider_id, "name": self.provider_name},
            "verifiedAt": self.verified_at,
        }
        for key, value in (("name", self.name), ("username", self.username), ("uri", self.uri)):
            if value is not None:
                entry[key] = value
        return entry


@dataclass(frozen=True)
class IdentityClaimsAggregation:
    issuer: str
    valid_from: str
    verified_identities: Sequence[VerifiedIdentity]
    credential_id: str
    sign: Callable[[bytes, Mapping[str, object]], bytes] | None = field(repr=False, default=None)

    def credential(self) -> dict[str, object]:
        if not self.verified_identities:
            raise IdentityProviderRequired(
                "An identity_claims_aggregation credential requires at least one verified identity "
                "issued by an identity provider"
            )
        return {
            "@context": [
                "https://www.w3.org/ns/credentials/v2",
                "https://cawg.io/identity/1.1/ica/context/",
            ],
            "type": ["VerifiableCredential", "IdentityClaimsAggregationCredential"],
            "id": self.credential_id,
            "issuer": self.issuer,
            "validFrom": self.valid_from,
            "credentialSubject": {
                "verifiedIdentities": [vi.to_map() for vi in self.verified_identities],
            },
        }


def build_ica_identity_assertion(
    referenced_assertions: Sequence[HashedUri],
    aggregation: IdentityClaimsAggregation,
    roles: Iterable[str] | None = None,
    pad1_length: int = 32,
) -> IdentityAssertion:
    signer_payload = build_signer_payload(referenced_assertions, SIG_TYPE_ICA, roles)
    if aggregation.sign is None:
        raise IdentityProviderRequired(
            "The identity_claims_aggregation signature MUST be produced by the identity "
            "provider that issued the credential. Supply a sign callback bound to that "
            "provider (an OIDC or eIDAS issuer); this engine will not fabricate one."
        )
    credential = aggregation.credential()
    signature = aggregation.sign(encode_signer_payload(signer_payload), credential)
    if not isinstance(signature, (bytes, bytearray)) or not signature:
        raise IdentityProviderRequired("The identity provider returned no signature")
    return IdentityAssertion(
        signer_payload=signer_payload,
        signature=bytes(signature),
        pad1=_pad(pad1_length),
        pad2=None,
        proof_level="externally_verified",
        proof_note=(
            "Identity asserted by "
            f"{credential['issuer']} over {len(aggregation.verified_identities)} verified "
            "identity record(s)."
        ),
    )
