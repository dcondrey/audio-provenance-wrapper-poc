from __future__ import annotations

import dataclasses
import datetime as dt
import hashlib
import hmac
import json
import logging
import math
import os
from pathlib import Path

from cryptography import x509
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

from daemon.audio_association import extract_feature_sequence
from daemon.common import canonical_json_bytes, sha256_file, utc_timestamp
from daemon.provenance.provider import (
    NOTHING_FOUND_NORMATIVE_NOTE,
    ProvenanceProvider,
    RevokedKeyError,
    SigningIdentity,
    SigningMaterial,
    VerificationState,
)

log = logging.getLogger(__name__)

DEFAULT_STORE = Path("~/.apw/provenance")
CERT_VALIDITY_DAYS = 365
ROOT_VALIDITY_DAYS = 3650
# Rotate before the leaf expires rather than at the moment it does, so a session
# started just inside the window cannot expire mid-export.
LEAF_RENEWAL_MARGIN_DAYS = 1

MARK_WINDOW_SECONDS = 0.25
MARK_WINDOW_COUNT = 24
MARK_MATCH_THRESHOLD = 0.90
MARK_BUCKET_TOLERANCE = 1
_RMS_BUCKETS = 64
_RMS_FLOOR_DB = -80.0
_ZCR_BUCKETS = 48
_ZCR_FLOOR_LOG10 = -5.0
_ZCR_CEIL_LOG10 = -0.30103
_CREST_BUCKETS = 24
_CREST_CEIL = 12.0


def _now() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def _append_only(path: Path, record: dict) -> None:
    """IMPORTANT: these logs are append-only evidence, so daemon.common.append_jsonl
    is deliberately not used: it rotates and drops oversize records, which would
    silently remove registry and signing history a verifier depends on."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("ab") as handle:
        handle.write(canonical_json_bytes(record) + b"\n")


def _write_private(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    os.chmod(path, 0o600)


def _key_id(public_key: ec.EllipticCurvePublicKey) -> str:
    der = public_key.public_bytes(
        serialization.Encoding.DER,
        serialization.PublicFormat.SubjectPublicKeyInfo,
    )
    return hashlib.sha256(der).hexdigest()[:16]


def validate_certificate_chain(
    chain_pem: bytes, trust_anchor_pem: bytes
) -> tuple[bool, str]:
    """Validate a leaf-then-issuer chain against a trust anchor.

    Enforces the profile the C2PA signer accepts: a CA:TRUE / keyCertSign root
    and a CA:FALSE leaf with critical digitalSignature, critical emailProtection,
    and an authorityKeyIdentifier. A single self-signed certificate is rejected
    outright.

    IMPORTANT: the authorityKeyIdentifier is not decorative. c2pa-rs rejects an
    otherwise well-formed leaf without it as "the certificate is invalid",
    measured against c2pa-python 0.90.15.
    """
    try:
        chain = x509.load_pem_x509_certificates(chain_pem)
        anchors = x509.load_pem_x509_certificates(trust_anchor_pem)
    except ValueError as exc:
        return False, f"chain could not be parsed: {exc}"
    if not chain:
        return False, "chain is empty"
    if not anchors:
        return False, "no trust anchor supplied"

    leaf = chain[0]
    if leaf.issuer == leaf.subject:
        return False, "the certificate was self-signed"

    try:
        basic = leaf.extensions.get_extension_for_class(x509.BasicConstraints).value
        usage = leaf.extensions.get_extension_for_class(x509.KeyUsage).value
        eku = leaf.extensions.get_extension_for_class(x509.ExtendedKeyUsage).value
        aki = leaf.extensions.get_extension_for_class(x509.AuthorityKeyIdentifier).value
    except x509.ExtensionNotFound as exc:
        return False, f"leaf is missing a required extension: {exc}"
    if basic.ca:
        return False, "leaf asserts CA:TRUE"
    if not usage.digital_signature:
        return False, "leaf keyUsage does not permit digitalSignature"
    if ExtendedKeyUsageOID.EMAIL_PROTECTION not in eku:
        return False, "leaf extendedKeyUsage does not include emailProtection"

    now = _now()
    for cert in chain:
        if cert.not_valid_before_utc > now or cert.not_valid_after_utc < now:
            return False, f"certificate outside its validity window: {cert.subject.rfc4514_string()}"

    issuer = next((cert for cert in chain[1:] + anchors if cert.subject == leaf.issuer), None)
    if issuer is None:
        return False, "leaf issuer is not present in the chain or trust anchors"
    if not any(anchor.subject == issuer.subject and anchor.public_key().public_numbers()
               == issuer.public_key().public_numbers() for anchor in anchors):
        return False, "chain does not terminate at a configured trust anchor"

    issuer_basic = issuer.extensions.get_extension_for_class(x509.BasicConstraints).value
    issuer_usage = issuer.extensions.get_extension_for_class(x509.KeyUsage).value
    if not issuer_basic.ca or not issuer_usage.key_cert_sign:
        return False, "issuer is not a certificate-signing CA"

    issuer_ski = issuer.extensions.get_extension_for_class(x509.SubjectKeyIdentifier).value
    if aki.key_identifier != issuer_ski.digest:
        return False, "leaf authorityKeyIdentifier does not match its issuer"

    try:
        issuer.public_key().verify(
            leaf.signature, leaf.tbs_certificate_bytes, ec.ECDSA(leaf.signature_hash_algorithm)
        )
    except InvalidSignature:
        return False, "leaf signature does not verify against its issuer"
    return True, "chain validates against the configured trust anchor"


def _bucket(unit_value: float, buckets: int) -> int:
    return int(max(0.0, min(1.0, unit_value)) * (buckets - 1))


def _descriptor(audio_path: Path) -> list[int]:
    """Quantise loudness, zero-crossing rate and crest per window.

    IMPORTANT: RMS and ZCR are bucketed in the log domain. Bucketed linearly on
    [0,1] they crowd into a handful of buckets and two unrelated tones quantise
    identically, which made the soft binding match anything.
    """
    sequence, _ = extract_feature_sequence(
        audio_path, MARK_WINDOW_SECONDS, max_windows=MARK_WINDOW_COUNT
    )
    descriptor: list[int] = []
    for feature in sequence:
        rms_db = 20.0 * math.log10(max(float(feature["rms"]), 1e-9))
        zcr_log = math.log10(max(float(feature["zcr"]), 1e-9))
        descriptor.append(
            _bucket((rms_db - _RMS_FLOOR_DB) / -_RMS_FLOOR_DB, _RMS_BUCKETS)
        )
        descriptor.append(
            _bucket(
                (zcr_log - _ZCR_FLOOR_LOG10) / (_ZCR_CEIL_LOG10 - _ZCR_FLOOR_LOG10),
                _ZCR_BUCKETS,
            )
        )
        descriptor.append(
            _bucket((float(feature["crest"]) - 1.0) / (_CREST_CEIL - 1.0), _CREST_BUCKETS)
        )
    return descriptor


def _descriptor_similarity(left: list[int], right: list[int]) -> float:
    """Fraction of descriptor dimensions agreeing within MARK_BUCKET_TOLERANCE.

    An averaged distance lets a large disagreement in one feature hide behind
    agreement in the others; per-dimension agreement does not.
    """
    if not left or not right:
        return 0.0
    length = min(len(left), len(right))
    agreeing = sum(
        1 for i in range(length) if abs(left[i] - right[i]) <= MARK_BUCKET_TOLERANCE
    )
    return agreeing / max(len(left), len(right))


# The fields a registry receipt signs over itself, excluding the signature.
_RECEIPT_SIGNATURE_FIELDS = (
    "registry_id", "content_sha256", "manifest_sha256", "mark_id", "key_id", "registered_at",
)


def _receipt_payload(record: dict) -> bytes:
    return b"apw-registry-receipt-v1" + canonical_json_bytes(
        {field: record.get(field) for field in _RECEIPT_SIGNATURE_FIELDS}
    )


def _match_proof_level(matched_by: str) -> str:
    if matched_by in {"content_sha256", "manifest_sha256"}:
        return "directly_observed"
    if matched_by == "mark_id":
        return "inferred"
    return "unknown_unobserved"


class LocalReferenceProvider(ProvenanceProvider):
    """File-backed reference implementation of the vault/mark/registry contract.

    Key custody lives under <store>/ca and <store>/keys, signing and revocation
    history in history.jsonl, the registry in registry.jsonl, and the soft
    binding side index in marks.jsonl. Every log is append-only.

    IMPORTANT: identities issued here are self-asserted. Their proof level is
    'user_declared'; no external party has attested to the creator's identity.
    """

    def __init__(
        self,
        store_dir: Path = DEFAULT_STORE,
        common_name: str = "Audio Provenance Wrapper Local Creator",
    ) -> None:
        self.store_dir = Path(store_dir).expanduser()
        self.common_name = common_name
        self.store_dir.mkdir(parents=True, exist_ok=True)
        self.history_path = self.store_dir / "history.jsonl"
        self.registry_path = self.store_dir / "registry.jsonl"
        self.marks_path = self.store_dir / "marks.jsonl"
        self._root_key, self._root_cert = self._load_or_create_root()
        self._leaf_key, self._leaf_cert = self._load_or_create_leaf()
        self._mark_key = self._load_or_create_mark_key()

    # --- key custody -----------------------------------------------------

    def _load_or_create_root(self) -> tuple[ec.EllipticCurvePrivateKey, x509.Certificate]:
        key_path = self.store_dir / "ca" / "root_key.pem"
        cert_path = self.store_dir / "ca" / "root_cert.pem"
        if key_path.is_file() and cert_path.is_file():
            key = serialization.load_pem_private_key(key_path.read_bytes(), password=None)
            return key, x509.load_pem_x509_certificate(cert_path.read_bytes())

        key = ec.generate_private_key(ec.SECP256R1())
        subject = x509.Name([
            x509.NameAttribute(NameOID.COMMON_NAME, "Audio Provenance Wrapper Local Root CA"),
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, "Audio Provenance Wrapper"),
        ])
        now = _now()
        cert = (
            x509.CertificateBuilder()
            .subject_name(subject)
            .issuer_name(subject)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - dt.timedelta(minutes=5))
            .not_valid_after(now + dt.timedelta(days=ROOT_VALIDITY_DAYS))
            .add_extension(x509.BasicConstraints(ca=True, path_length=1), critical=True)
            .add_extension(
                x509.KeyUsage(
                    digital_signature=False, content_commitment=False, key_encipherment=False,
                    data_encipherment=False, key_agreement=False, key_cert_sign=True,
                    crl_sign=True, encipher_only=False, decipher_only=False,
                ),
                critical=True,
            )
            .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
            .sign(key, hashes.SHA256())
        )
        _write_private(key_path, key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        ))
        cert_path.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        log.info("Created local provenance root CA at %s", cert_path)
        return key, cert

    def _leaf_unusable(self, key_id: str, cert: x509.Certificate) -> str | None:
        """Why the stored leaf cannot sign, or None if it can."""
        try:
            if self._meta(key_id).get("revoked"):
                return "the key is revoked"
        except (KeyError, OSError, json.JSONDecodeError):
            return "its custody metadata is missing or unreadable"
        now = _now()
        if cert.not_valid_before_utc > now:
            return "the certificate is not yet valid"
        if cert.not_valid_after_utc < now + dt.timedelta(days=LEAF_RENEWAL_MARGIN_DAYS):
            return "the certificate has expired or expires within the renewal margin"
        return None

    def _load_or_create_leaf(self) -> tuple[ec.EllipticCurvePrivateKey, x509.Certificate]:
        """Return a usable leaf, issuing a replacement when the stored one is not.

        IMPORTANT: this used to reload keys/active.json unconditionally. A store
        older than CERT_VALIDITY_DAYS, or one whose active key had been revoked,
        then handed back material c2pa-rs refuses at signing time, and every
        export from that day on silently carried no C2PA claim at all with only
        a log line as the signal.
        """
        key_dir = self.store_dir / "keys"
        active = key_dir / "active.json"
        if active.is_file():
            try:
                record = json.loads(active.read_text())
                key_id = str(record["key_id"])
                key = serialization.load_pem_private_key(
                    (key_dir / key_id / "leaf_key.pem").read_bytes(), password=None
                )
                cert = x509.load_pem_x509_certificate(
                    (key_dir / key_id / "leaf_cert.pem").read_bytes()
                )
            except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
                log.warning("Active provenance leaf could not be loaded (%s); issuing a replacement", exc)
            else:
                reason = self._leaf_unusable(key_id, cert)
                if reason is None:
                    return key, cert
                log.warning(
                    "Active provenance leaf %s is unusable (%s); issuing a replacement",
                    key_id, reason,
                )
        return self._issue_leaf()

    def _issue_leaf(self) -> tuple[ec.EllipticCurvePrivateKey, x509.Certificate]:
        key_dir = self.store_dir / "keys"
        active = key_dir / "active.json"
        key = ec.generate_private_key(ec.SECP256R1())
        now = _now()
        cert = (
            x509.CertificateBuilder()
            .subject_name(x509.Name([
                x509.NameAttribute(NameOID.COMMON_NAME, self.common_name),
                x509.NameAttribute(NameOID.ORGANIZATION_NAME, "Audio Provenance Wrapper"),
            ]))
            .issuer_name(self._root_cert.subject)
            .public_key(key.public_key())
            .serial_number(x509.random_serial_number())
            .not_valid_before(now - dt.timedelta(minutes=5))
            .not_valid_after(now + dt.timedelta(days=CERT_VALIDITY_DAYS))
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(
                x509.KeyUsage(
                    digital_signature=True, content_commitment=False, key_encipherment=False,
                    data_encipherment=False, key_agreement=False, key_cert_sign=False,
                    crl_sign=False, encipher_only=False, decipher_only=False,
                ),
                critical=True,
            )
            .add_extension(
                x509.ExtendedKeyUsage([ExtendedKeyUsageOID.EMAIL_PROTECTION]), critical=True
            )
            .add_extension(
                x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False
            )
            .add_extension(
                x509.AuthorityKeyIdentifier.from_issuer_public_key(self._root_cert.public_key()),
                critical=False,
            )
            .sign(self._root_key, hashes.SHA256())
        )
        key_id = _key_id(key.public_key())
        _write_private(key_dir / key_id / "leaf_key.pem", key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        ))
        (key_dir / key_id / "leaf_cert.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        (key_dir / key_id / "meta.json").write_text(json.dumps({
            "key_id": key_id,
            "common_name": self.common_name,
            "created_at": utc_timestamp(),
            "revoked": False,
        }, indent=2))
        active.write_text(json.dumps({"key_id": key_id}, indent=2))
        _append_only(self.history_path, {
            "event": "issue",
            "key_id": key_id,
            "at": utc_timestamp(),
            "not_valid_after": cert.not_valid_after_utc.isoformat(),
            "apw:proof_level": "directly_observed",
        })
        log.info("Issued local provenance leaf certificate %s", key_id)
        return key, cert

    def _load_or_create_mark_key(self) -> bytes:
        path = self.store_dir / "mark_key.bin"
        if path.is_file():
            return path.read_bytes()
        key = os.urandom(32)
        _write_private(path, key)
        return key

    def _meta_path(self, key_id: str) -> Path:
        return self.store_dir / "keys" / key_id / "meta.json"

    def _meta(self, key_id: str) -> dict:
        path = self._meta_path(key_id)
        if not path.is_file():
            raise KeyError(f"unknown key_id: {key_id}")
        return json.loads(path.read_text())

    @property
    def key_id(self) -> str:
        return _key_id(self._leaf_key.public_key())

    def trust_anchor_pem(self) -> bytes:
        return self._root_cert.public_bytes(serialization.Encoding.PEM)

    def chain_pem(self) -> bytes:
        return self._leaf_cert.public_bytes(serialization.Encoding.PEM) + self.trust_anchor_pem()

    # --- ProvenanceProvider ---------------------------------------------

    def identity(self) -> dict:
        meta = self._meta(self.key_id)
        identity = SigningIdentity(
            key_id=self.key_id,
            subject_common_name=self.common_name,
            algorithm="es256",
            identity_evidence="locally_generated_key_no_external_attestation",
            revoked=bool(meta.get("revoked")),
            created_at_ms=int(self._leaf_cert.not_valid_before_utc.timestamp() * 1000),
            proof_level="user_declared",
        )
        fields = dataclasses.asdict(identity)
        proof_level = fields.pop("proof_level")
        return {
            **fields,
            "trust_anchor_sha256": hashlib.sha256(self.trust_anchor_pem()).hexdigest(),
            "apw:proof_level": proof_level,
            "limits": (
                "The certificate binds a key to a name this machine chose. No "
                "external party verified that the name belongs to the creator."
            ),
        }

    def issue_signing_material(self) -> SigningMaterial:
        meta = self._meta(self.key_id)
        if meta.get("revoked"):
            raise RevokedKeyError(f"key {self.key_id} is revoked and cannot issue signing material")
        return SigningMaterial(
            key_id=self.key_id,
            algorithm="es256",
            certificate_chain_pem=self.chain_pem(),
            private_key_handle=self._leaf_key.private_bytes(
                serialization.Encoding.PEM,
                serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption(),
            ),
            trust_anchor_pem=self.trust_anchor_pem(),
            proof_level="user_declared",
        )

    def sign_claim(self, payload: bytes) -> bytes:
        key_id = self.key_id
        if self._meta(key_id).get("revoked"):
            raise RevokedKeyError(f"key {key_id} is revoked and cannot sign new claims")
        signature = self._leaf_key.sign(payload, ec.ECDSA(hashes.SHA256()))
        _append_only(self.history_path, {
            "event": "sign",
            "key_id": key_id,
            "at": utc_timestamp(),
            "payload_sha256": hashlib.sha256(payload).hexdigest(),
            "signature_hex": signature.hex(),
            "apw:proof_level": "directly_observed",
        })
        return signature

    def embed_mark(self, audio_path: Path, payload: dict) -> dict:
        """Attach a keyed feature-domain soft binding via a local side index.

        IMPORTANT: this does not modify a single audio sample. The 'mark' is an
        HMAC over a coarse feature descriptor of the asset, stored in a local
        side index. It is a working, deterministic soft binding and nothing more.
        """
        audio_path = Path(audio_path)
        descriptor = _descriptor(audio_path)
        if not descriptor:
            raise ValueError(f"asset is too short to describe: {audio_path}")
        content_sha256 = sha256_file(audio_path)
        payload_bytes = canonical_json_bytes(payload)
        mark_id = hmac.new(
            self._mark_key,
            b"apw-soft-binding-v1"
            + canonical_json_bytes(descriptor)
            + payload_bytes
            + content_sha256.encode(),
            hashlib.sha256,
        ).hexdigest()
        record = {
            "mark_id": mark_id,
            "descriptor": descriptor,
            "content_sha256": content_sha256,
            "payload": payload,
            "created_at": utc_timestamp(),
        }
        _append_only(self.marks_path, record)
        return {
            "mark_id": mark_id,
            "content_sha256": content_sha256,
            "asset_modified": False,
            "mechanism": (
                "keyed HMAC over a quantised RMS/ZCR/crest descriptor of the first "
                f"{MARK_WINDOW_COUNT} windows of {MARK_WINDOW_SECONDS}s, recorded in a "
                "local append-only side index"
            ),
            "apw:proof_level": "inferred",
            "limits": [
                "No audio samples are altered, so inaudibility is not claimed and not applicable.",
                "Survival through lossy transcoding, resampling, or re-recording is unmeasured.",
                "Recovery requires this machine's side index; the mark is not carried by the asset.",
                "Only uncompressed PCM WAV and AIFF assets can be described.",
                f"Recovery is a nearest-descriptor match: at least {MARK_MATCH_THRESHOLD:.0%} of "
                f"descriptor dimensions within {MARK_BUCKET_TOLERANCE} bucket. It is evidence of "
                "resemblance, not of identity.",
                "Two takes that are near-identical in loudness, zero-crossing rate and crest "
                "quantise alike and will collide; a verdict reached through the mark alone is "
                "reported with proof level 'inferred'.",
            ],
        }

    def recover_mark(self, audio_path: Path) -> dict | None:
        audio_path = Path(audio_path)
        if not self.marks_path.is_file():
            return None
        try:
            descriptor = _descriptor(audio_path)
        except ValueError:
            return None
        if not descriptor:
            return None
        best: dict | None = None
        best_similarity = 0.0
        for line in self.marks_path.read_text().splitlines():
            if not line.strip():
                continue
            record = json.loads(line)
            similarity = _descriptor_similarity(descriptor, record["descriptor"])
            if similarity > best_similarity:
                best, best_similarity = record, similarity
        if best is None or best_similarity < MARK_MATCH_THRESHOLD:
            return None
        return {
            "mark_id": best["mark_id"],
            "payload": best["payload"],
            "registered_content_sha256": best["content_sha256"],
            "observed_content_sha256": sha256_file(audio_path),
            "match_similarity": round(best_similarity, 6),
            "apw:proof_level": "inferred",
        }

    def register(self, manifest: dict) -> dict:
        content_sha256 = str(manifest.get("content_sha256", ""))
        if len(content_sha256) != 64:
            raise ValueError("manifest must carry a 64-character content_sha256 for registration")
        manifest_bytes = canonical_json_bytes(manifest)
        manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()
        signature = self.sign_claim(manifest_bytes)
        receipt = {
            "registry_id": hashlib.sha256(
                manifest_sha256.encode() + content_sha256.encode()
            ).hexdigest()[:32],
            "content_sha256": content_sha256,
            "manifest_sha256": manifest_sha256,
            "mark_id": manifest.get("mark_id"),
            "key_id": self.key_id,
            "chain_pem": self.chain_pem().decode(),
            "signature_hex": signature.hex(),
            "registered_at": utc_timestamp(),
            "apw:proof_level": "directly_observed",
        }
        # IMPORTANT: registry.jsonl is a plain append-only text file, so anyone
        # who can write it can append a record carrying a chain_pem copied from a
        # genuine one and an arbitrary content_sha256. The receipt signature binds
        # the record's own fields to the leaf key, so a copied chain is not enough.
        receipt["receipt_signature_hex"] = self.sign_claim(_receipt_payload(receipt)).hex()
        _append_only(self.registry_path, receipt)
        return receipt

    def _registry_records(self) -> list[dict]:
        if not self.registry_path.is_file():
            return []
        return [
            json.loads(line)
            for line in self.registry_path.read_text().splitlines()
            if line.strip()
        ]

    def _lookup(
        self, content_sha256: str, mark_id: str | None, manifest_sha256: str | None
    ) -> tuple[dict | None, str]:
        """Return the registry record and which key matched it.

        The matched key drives the reported proof level: a content or manifest
        digest is an exact match, a mark is a similarity match and can only ever
        support an 'inferred' claim.
        """
        records = self._registry_records()
        for record in reversed(records):
            if record["content_sha256"] == content_sha256:
                return record, "content_sha256"
        for record in reversed(records):
            if manifest_sha256 and record["manifest_sha256"] == manifest_sha256:
                return record, "manifest_sha256"
        for record in reversed(records):
            if mark_id and record.get("mark_id") == mark_id:
                return record, "mark_id"
        return None, "none"

    def _claim_trust(self, record: dict, manifest_bytes: bytes | None) -> tuple[bool, str]:
        """Whether the registry record is trustworthy, and why.

        IMPORTANT: this used to return the string "claim signature and certificate
        chain validate" on both branches, so verify(asset, None) reported that a
        signature validated when none had been checked at all.
        """
        ok, reason = validate_certificate_chain(
            record["chain_pem"].encode(), self.trust_anchor_pem()
        )
        if not ok:
            return False, reason
        try:
            if self._meta(record["key_id"]).get("revoked"):
                return False, "signing key is revoked"
        except KeyError:
            return False, "signing key is not present in this custody store"

        try:
            leaf = x509.load_pem_x509_certificates(record["chain_pem"].encode())[0]
            public_key = leaf.public_key()
        except (ValueError, IndexError):
            return False, "registry record carries no usable signing certificate"

        receipt_signature = record.get("receipt_signature_hex")
        if not isinstance(receipt_signature, str) or not receipt_signature:
            return False, "registry record carries no receipt signature over its own fields"
        try:
            public_key.verify(
                bytes.fromhex(receipt_signature),
                _receipt_payload(record),
                ec.ECDSA(hashes.SHA256()),
            )
        except (InvalidSignature, ValueError):
            return False, "registry receipt signature does not verify"

        if manifest_bytes is None:
            return True, (
                "the registry receipt signature and certificate chain validate; no manifest "
                "was supplied, so the claim signature over it was not checked"
            )
        if hashlib.sha256(manifest_bytes).hexdigest() != record["manifest_sha256"]:
            return False, "supplied manifest does not match the registered manifest digest"
        try:
            public_key.verify(
                bytes.fromhex(record["signature_hex"]), manifest_bytes,
                ec.ECDSA(hashes.SHA256()),
            )
        except (InvalidSignature, ValueError):
            return False, "claim signature does not verify"
        return True, "claim signature and certificate chain validate"

    def verify(self, asset_path: Path, manifest_bytes: bytes | None) -> dict:
        asset_path = Path(asset_path)
        content_sha256 = sha256_file(asset_path)
        mark = self.recover_mark(asset_path)
        manifest_sha256 = (
            hashlib.sha256(manifest_bytes).hexdigest() if manifest_bytes is not None else None
        )
        record, matched_by = self._lookup(
            content_sha256, mark["mark_id"] if mark else None, manifest_sha256
        )

        if record is None and mark is None and manifest_bytes is None:
            state, reason = VerificationState.NOTHING_FOUND, "no mark and no registry record"
        elif record is None:
            state = VerificationState.MARK_FOUND_CLAIM_NOT_TRUSTED
            reason = "provenance data was presented but no registry record backs it"
        else:
            trusted, reason = self._claim_trust(record, manifest_bytes)
            if not trusted:
                state = VerificationState.MARK_FOUND_CLAIM_NOT_TRUSTED
            elif record["content_sha256"] != content_sha256:
                state = VerificationState.REGISTERED_BUT_CHANGED
                reason = "asset bytes differ from the registered content digest"
            else:
                state = VerificationState.VERIFIED

        return {
            "state": state.value,
            "reason": reason,
            "content_sha256": content_sha256,
            "registry_id": record["registry_id"] if record else None,
            "mark": mark,
            "matched_by": matched_by,
            "apw:proof_level": _match_proof_level(matched_by),
            "normative_note": NOTHING_FOUND_NORMATIVE_NOTE,
        }

    def revoke(self, key_id: str) -> dict:
        """Revoke a key and, when it was the active one, issue its replacement.

        IMPORTANT: revocation left keys/active.json pointing at the revoked key
        with no code path anywhere that issued a successor, so every later export
        recorded an unavailable C2PA claim for the life of the store. Prior
        signing history is retained, which is the interface's actual requirement.
        """
        meta = self._meta(key_id)
        revoked_at = utc_timestamp()
        meta["revoked"] = True
        meta["revoked_at"] = revoked_at
        self._meta_path(key_id).write_text(json.dumps(meta, indent=2))
        record = {
            "event": "revoke",
            "key_id": key_id,
            "at": revoked_at,
            "retained_signing_records": len(self.signing_history(key_id)),
            "apw:proof_level": "directly_observed",
        }
        _append_only(self.history_path, record)
        log.info(
            "Revoked provenance key %s; signing history retained. A replacement leaf is "
            "issued the next time this store is opened.",
            key_id,
        )
        return record

    def signing_history(self, key_id: str) -> list[dict]:
        if not self.history_path.is_file():
            return []
        history: list[dict] = []
        for line in self.history_path.read_text().splitlines():
            if not line.strip():
                continue
            record = json.loads(line)
            if record.get("key_id") == key_id and record.get("event") == "sign":
                history.append(record)
        return history
