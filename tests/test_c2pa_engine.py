from __future__ import annotations

import datetime
import struct
from pathlib import Path

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

from daemon.c2pa_engine import identity, manifest, signer, verifier
from tests.audio_files import write_aiff, write_wav

# IMPORTANT: c2pa-rs rejects a signing certificate whose subject carries only a common name
# (it reports claimSignature.mismatch), so the C2PA certificate profile's organizationName is
# required on both certificates here.
def _name(common_name: str) -> x509.Name:
    return x509.Name(
        [
            x509.NameAttribute(NameOID.COMMON_NAME, common_name),
            x509.NameAttribute(NameOID.ORGANIZATION_NAME, "APW Test Signer"),
            x509.NameAttribute(NameOID.COUNTRY_NAME, "US"),
        ]
    )


def make_chain(label: str = "apw") -> tuple[bytes, bytes, bytes]:
    now = datetime.datetime.now(datetime.timezone.utc)
    not_before = now - datetime.timedelta(days=1)
    not_after = now + datetime.timedelta(days=365)

    root_key = ec.generate_private_key(ec.SECP256R1())
    root_name = _name(f"{label} root")
    root = (
        x509.CertificateBuilder()
        .subject_name(root_name)
        .issuer_name(root_name)
        .public_key(root_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(not_before)
        .not_valid_after(not_after)
        .add_extension(x509.BasicConstraints(True, None), critical=True)
        .add_extension(
            x509.KeyUsage(False, False, False, False, False, True, True, False, False), critical=True
        )
        .add_extension(x509.SubjectKeyIdentifier.from_public_key(root_key.public_key()), critical=False)
        .add_extension(
            x509.AuthorityKeyIdentifier.from_issuer_public_key(root_key.public_key()), critical=False
        )
        .sign(root_key, hashes.SHA256())
    )

    leaf_key = ec.generate_private_key(ec.SECP256R1())
    leaf = (
        x509.CertificateBuilder()
        .subject_name(_name(f"{label} leaf"))
        .issuer_name(root_name)
        .public_key(leaf_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(not_before)
        .not_valid_after(not_after)
        .add_extension(x509.BasicConstraints(False, None), critical=True)
        .add_extension(
            x509.KeyUsage(True, False, False, False, False, False, False, False, False), critical=True
        )
        .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.EMAIL_PROTECTION]), critical=True)
        .add_extension(x509.SubjectKeyIdentifier.from_public_key(leaf_key.public_key()), critical=False)
        .add_extension(
            x509.AuthorityKeyIdentifier.from_issuer_public_key(root_key.public_key()), critical=False
        )
        .sign(root_key, hashes.SHA256())
    )

    chain_pem = leaf.public_bytes(serialization.Encoding.PEM) + root.public_bytes(
        serialization.Encoding.PEM
    )
    key_pem = leaf_key.private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    )
    return chain_pem, key_pem, root.public_bytes(serialization.Encoding.PEM)


@pytest.fixture(scope="module")
def chain() -> tuple[bytes, bytes, bytes]:
    return make_chain()


@pytest.fixture
def spec(tmp_path: Path) -> manifest.ManifestSpec:
    sample = write_wav(tmp_path / "sample.wav", 500)
    return manifest.ManifestSpec(
        title="mix.wav",
        actions=[manifest.ObservedAction(edit_type="clip_paste", proof_level="inferred", confidence=0.8)],
        ingredients=[
            manifest.Ingredient(
                title="stem-1",
                relationship="componentOf",
                proof_level="directly_observed",
                sha256="a" * 64,
            ),
            manifest.unobserved_ingredient("sample.wav", sample),
        ],
    )


def test_signed_wav_round_trips_to_verified_with_supplied_anchor(tmp_path, chain, spec):
    chain_pem, key_pem, root_pem = chain
    src = write_wav(tmp_path / "mix.wav")
    dest = tmp_path / "mix-signed.wav"

    result = signer.sign_wav(src, dest, manifest.build_manifest(spec), signer.build_signer(chain_pem, key_pem))
    assert result.mode == "embedded"
    original = src.read_bytes()
    signed = dest.read_bytes()
    assert signed[result.binding["excluded_region"]["start"] :]
    # Only the RIFF size field at offset 4 may differ; every audio byte is carried through.
    differing = [i for i in range(len(original)) if original[i] != signed[i]]
    assert differing == [4, 5, 6, 7] or set(differing) <= {4, 5, 6, 7}

    verified = verifier.verify_asset(dest, "audio/wav", trust_anchors_pem=root_pem)
    assert verified.state == verifier.VERIFIED
    assert verified.failure_codes == ()
    assert manifest.UNOBSERVED_ASSERTION_LABEL in verified.assertion_labels
    assert manifest.ACTIONS_ASSERTION_LABEL in verified.assertion_labels
    relationships = {ing["relationship"] for ing in verified.manifest["ingredients"]}
    assert relationships == {"componentOf", "inputTo"}
    unobserved = next(
        ing for ing in verified.manifest["ingredients"] if ing["relationship"] == "inputTo"
    )
    assert unobserved["metadata"]["apw:proof_level"] == "unknown_unobserved"
    assert unobserved["metadata"]["apw:sha256"] == manifest.sha256_file(tmp_path / "sample.wav")


def test_flipped_audio_byte_is_registered_but_changed(tmp_path, chain, spec):
    chain_pem, key_pem, root_pem = chain
    src = write_wav(tmp_path / "mix.wav")
    dest = tmp_path / "mix-signed.wav"
    signer.sign_wav(src, dest, manifest.build_manifest(spec), signer.build_signer(chain_pem, key_pem))

    payload = bytearray(dest.read_bytes())
    payload[src.stat().st_size // 2] ^= 0xFF
    tampered = tmp_path / "mix-tampered.wav"
    tampered.write_bytes(bytes(payload))

    changed = verifier.verify_asset(tampered, "audio/wav", trust_anchors_pem=root_pem)
    assert changed.state == verifier.REGISTERED_BUT_CHANGED
    assert "assertion.dataHash.mismatch" in changed.failure_codes


def test_chain_outside_the_anchor_list_is_not_trusted(tmp_path, chain, spec):
    chain_pem, key_pem, _root_pem = chain
    _other_chain, _other_key, other_root = make_chain("unrelated")
    src = write_wav(tmp_path / "mix.wav")
    dest = tmp_path / "mix-signed.wav"
    signer.sign_wav(src, dest, manifest.build_manifest(spec), signer.build_signer(chain_pem, key_pem))

    untrusted = verifier.verify_asset(dest, "audio/wav", trust_anchors_pem=other_root)
    assert untrusted.state == verifier.MARK_FOUND_CLAIM_NOT_TRUSTED
    assert "signingCredential.untrusted" in untrusted.failure_codes


def test_unsigned_asset_is_nothing_found(tmp_path):
    src = write_wav(tmp_path / "bare.wav")
    assert verifier.verify_asset(src, "audio/wav").state == verifier.NOTHING_FOUND


def test_aiff_routes_to_a_whole_file_sidecar(tmp_path, chain, spec):
    chain_pem, key_pem, root_pem = chain
    src = write_aiff(tmp_path / "mix.aiff")
    spec.mime = signer.AIFF_MIME

    result = signer.sign_asset(
        src, tmp_path / "mix-signed.aiff", manifest.build_manifest(spec), signer.build_signer(chain_pem, key_pem)
    )
    assert result.mode == "sidecar"
    assert result.manifest_path.suffix == ".c2pa"
    assert result.binding["exclusions"] == []
    assert result.binding["container_rewrite_tolerance"] == "none"

    verified = verifier.verify_asset(
        src, signer.AIFF_MIME, trust_anchors_pem=root_pem, sidecar_manifest=result.manifest_path
    )
    assert verified.state == verifier.VERIFIED

    original = src.read_bytes()
    # The sidecar binding claims zero exclusions, so a flip anywhere - audio payload, the COMM
    # header describing it, or the final byte - has to break it.
    for offset in (len(original) // 2, 20, len(original) - 1):
        payload = bytearray(original)
        payload[offset] ^= 0xFF
        tampered = tmp_path / f"mix-tampered-{offset}.aiff"
        tampered.write_bytes(bytes(payload))
        changed = verifier.verify_asset(
            tampered, signer.AIFF_MIME, trust_anchors_pem=root_pem, sidecar_manifest=result.manifest_path
        )
        assert changed.state == verifier.REGISTERED_BUT_CHANGED, offset
        assert "assertion.dataHash.mismatch" in changed.failure_codes


def test_float32_wav_is_refused_with_an_actionable_message(tmp_path):
    src = write_wav(tmp_path / "float.wav", 1000, float32=True)
    with pytest.raises(signer.UnsupportedAssetError) as excinfo:
        signer.detect_format(src)
    assert "16-bit PCM WAV" in str(excinfo.value)


def test_float32_extensible_wav_is_refused(tmp_path):
    data = b"\x00" * 64
    fmt = struct.pack("<HHIIHH", 0xFFFE, 1, 44100, 176400, 4, 32) + struct.pack("<H", 22)
    fmt += struct.pack("<HI", 32, 3) + struct.pack("<H", 3) + b"\x00" * 14
    body = b"WAVEfmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    src = tmp_path / "ext.wav"
    src.write_bytes(b"RIFF" + struct.pack("<I", len(body) + 4) + body)
    with pytest.raises(signer.UnsupportedAssetError, match="16-bit PCM WAV"):
        signer.detect_format(src)


def test_ingredient_without_provenance_must_still_record_a_hash():
    with pytest.raises(manifest.ManifestError, match="sha256"):
        manifest.Ingredient(title="mystery", relationship="inputTo", proof_level="unknown_unobserved")


def test_manifest_always_declares_the_unobserved_assertion():
    built = manifest.build_manifest(manifest.ManifestSpec(title="mix.wav"))
    assert manifest.UNOBSERVED_ASSERTION_LABEL in manifest.assertion_labels(built)
    unobserved = next(
        a for a in built["assertions"] if a["label"] == manifest.UNOBSERVED_ASSERTION_LABEL
    )
    assert unobserved["data"]["items"][0]["claim"] == "full_daw_provenance"
    assert unobserved["data"]["items"][0]["apw:proof_level"] == "unknown_unobserved"


def _hard_binding() -> identity.HashedUri:
    return identity.HashedUri(url="self#jumbf=c2pa.assertions/c2pa.hash.data", hash=b"\x01" * 32)


def test_signer_payload_uses_rfc8949_bytewise_key_order():
    payload = identity.build_signer_payload(
        [_hard_binding()], identity.SIG_TYPE_X509_COSE, roles=["cawg.creator"]
    )
    encoded = identity.encode_signer_payload(payload)
    assert encoded.index(b"role") < encoded.index(b"sig_type") < encoded.index(b"referenced_assertions")
    assert encoded[0] == 0xA3


def test_referenced_assertions_must_include_exactly_one_hard_binding_and_no_self_reference():
    other = identity.HashedUri(url="self#jumbf=c2pa.assertions/c2pa.actions.v2", hash=b"\x02" * 32)
    with pytest.raises(identity.IdentityAssertionError, match="hard binding"):
        identity.build_signer_payload([other], identity.SIG_TYPE_X509_COSE)
    second = identity.HashedUri(url="self#jumbf=c2pa.assertions/c2pa.hash.data__1", hash=b"\x03" * 32)
    with pytest.raises(identity.IdentityAssertionError, match="second hard binding"):
        identity.build_signer_payload([_hard_binding(), second], identity.SIG_TYPE_X509_COSE)
    with pytest.raises(identity.IdentityAssertionError, match="twice"):
        identity.build_signer_payload([_hard_binding(), _hard_binding()], identity.SIG_TYPE_X509_COSE)
    myself = identity.HashedUri(url="self#jumbf=c2pa.assertions/cawg.identity", hash=b"\x04" * 32)
    with pytest.raises(identity.IdentityAssertionError, match="itself"):
        identity.build_signer_payload([_hard_binding(), myself], identity.SIG_TYPE_X509_COSE)


def test_x509_identity_assertion_signs_the_signer_payload_not_the_claim(chain):
    chain_pem, key_pem, _root_pem = chain
    assertion = identity.build_x509_identity_assertion(
        [_hard_binding()], chain_pem, key_pem, roles=["cawg.creator"]
    )
    assert assertion.proof_level == "user_declared"
    assert assertion.pad1 == b"\x00" * 32
    leaf = identity.verify_signer_payload_x509(assertion.signature, assertion.signer_payload)
    assert leaf.subject.get_attributes_for_oid(NameOID.ORGANIZATION_NAME)

    forged = dict(assertion.signer_payload)
    forged["role"] = ["cawg.publisher"]
    with pytest.raises(identity.IdentityAssertionError, match="mismatch"):
        identity.verify_signer_payload_x509(assertion.signature, forged)


def test_identity_claims_aggregation_requires_a_real_provider():
    aggregation = identity.IdentityClaimsAggregation(
        issuer="did:web:issuer.example",
        valid_from="2026-08-31T00:00:00Z",
        verified_identities=[
            identity.VerifiedIdentity(
                type="cawg.social_media",
                provider_id="https://issuer.example",
                provider_name="Example IdP",
                verified_at="2026-08-30T00:00:00Z",
                username="producer",
            )
        ],
        credential_id="urn:uuid:0f1d2c3b-4a59-4687-b8c9-6d0e1f2a3b4c",
    )
    with pytest.raises(identity.IdentityProviderRequired, match="identity provider"):
        identity.build_ica_identity_assertion([_hard_binding()], aggregation)

    signed = identity.IdentityClaimsAggregation(
        issuer=aggregation.issuer,
        valid_from=aggregation.valid_from,
        verified_identities=aggregation.verified_identities,
        credential_id=aggregation.credential_id,
        sign=lambda payload, credential: b"provider-signed:" + payload[:8],
    )
    assertion = identity.build_ica_identity_assertion([_hard_binding()], signed, roles=["cawg.producer"])
    assert assertion.signer_payload["sig_type"] == identity.SIG_TYPE_ICA
    assert assertion.proof_level == "externally_verified"

    empty = identity.IdentityClaimsAggregation(
        issuer=aggregation.issuer,
        valid_from=aggregation.valid_from,
        verified_identities=[],
        credential_id=aggregation.credential_id,
        sign=lambda payload, credential: b"x",
    )
    with pytest.raises(identity.IdentityProviderRequired, match="verified identity"):
        identity.build_ica_identity_assertion([_hard_binding()], empty)


def test_build_signer_rejects_a_lone_self_signed_certificate(chain):
    chain_pem, key_pem, root_pem = chain
    with pytest.raises(signer.SigningError, match="self-signed"):
        signer.build_signer(root_pem, key_pem)


def test_parent_ingredient_becomes_the_opening_action(tmp_path, chain):
    chain_pem, key_pem, root_pem = chain
    parent = write_wav(tmp_path / "previous-mix.wav", 500)
    spec = manifest.ManifestSpec(
        title="mix.wav",
        actions=[
            manifest.ObservedAction(
                edit_type="clip_paste",
                proof_level="inferred",
                ingredient_ids=["stem-1"],
            )
        ],
        ingredients=[
            manifest.Ingredient(
                title="previous-mix.wav",
                relationship="parentOf",
                proof_level="inferred",
                source_path=parent,
            ),
            manifest.Ingredient(
                title="stem-1",
                relationship="componentOf",
                proof_level="directly_observed",
                sha256="b" * 64,
            ),
        ],
    )
    built = manifest.build_manifest(spec)
    actions = built["assertions"][0]["data"]["actions"]
    assert actions[0]["action"] == "c2pa.opened"
    assert actions[0]["parameters"]["ingredientIds"] == ["previous-mix.wav"]
    assert actions[1]["action"] == "c2pa.placed"
    assert actions[1]["parameters"]["ingredientIds"] == ["stem-1"]
    assert len(actions) == 2

    src = write_wav(tmp_path / "mix.wav")
    dest = tmp_path / "mix-signed.wav"
    signer.sign_wav(src, dest, built, signer.build_signer(chain_pem, key_pem))
    assert verifier.verify_asset(dest, "audio/wav", trust_anchors_pem=root_pem).state == verifier.VERIFIED


def test_action_that_places_nothing_is_not_reported_as_placed():
    built = manifest.build_manifest(
        manifest.ManifestSpec(
            title="mix.wav",
            actions=[manifest.ObservedAction(edit_type="clip_paste", proof_level="inferred")],
        )
    )
    actions = built["assertions"][0]["data"]["actions"]
    assert [a["action"] for a in actions] == ["c2pa.created", "c2pa.edited"]
    assert actions[1]["parameters"]["apw:edit_type"] == "clip_paste"


def test_duplicate_ingredient_ids_are_refused():
    duplicate = [
        manifest.Ingredient(
            title="kick.wav", relationship="componentOf", proof_level="inferred", sha256="a" * 64
        ),
        manifest.Ingredient(
            title="kick.wav", relationship="componentOf", proof_level="inferred", sha256="b" * 64
        ),
    ]
    with pytest.raises(manifest.ManifestError, match="Duplicate ingredient id"):
        manifest.build_manifest(manifest.ManifestSpec(title="mix.wav", ingredients=duplicate))


def test_unclassified_and_removal_edits_stay_valid_under_claim_v2(tmp_path, chain):
    chain_pem, key_pem, root_pem = chain
    spec = manifest.ManifestSpec(
        title="mix.wav",
        actions=[
            manifest.ObservedAction(edit_type="rewire_thing", proof_level="inferred"),
            manifest.ObservedAction(
                edit_type="clip_delete", proof_level="inferred", ingredient_ids=["stem-1"]
            ),
        ],
        ingredients=[
            manifest.Ingredient(
                title="stem-1",
                relationship="componentOf",
                proof_level="directly_observed",
                sha256="c" * 64,
            )
        ],
    )
    built = manifest.build_manifest(spec)
    assert [a["action"] for a in built["assertions"][0]["data"]["actions"]] == [
        "c2pa.created",
        "c2pa.unknown",
        "c2pa.removed",
    ]
    src = write_wav(tmp_path / "mix.wav")
    dest = tmp_path / "mix-signed.wav"
    signer.sign_wav(src, dest, built, signer.build_signer(chain_pem, key_pem))
    assert verifier.verify_asset(dest, "audio/wav", trust_anchors_pem=root_pem).state == verifier.VERIFIED


def test_actions_cannot_reference_an_unknown_ingredient():
    with pytest.raises(manifest.ManifestError, match="unknown ingredient ids"):
        manifest.build_manifest(
            manifest.ManifestSpec(
                title="mix.wav",
                actions=[
                    manifest.ObservedAction(
                        edit_type="clip_paste", proof_level="inferred", ingredient_ids=["ghost"]
                    )
                ],
            )
        )


def test_float_wav_is_refused_even_when_fmt_follows_a_large_metadata_chunk(tmp_path):
    junk = b"\x00" * 6000
    fmt = struct.pack("<HHIIHH", 3, 1, 44100, 176400, 4, 32)
    data = b"\x00" * 64
    body = (
        b"WAVE"
        + b"LIST"
        + struct.pack("<I", len(junk))
        + junk
        + b"fmt "
        + struct.pack("<I", len(fmt))
        + fmt
        + b"data"
        + struct.pack("<I", len(data))
        + data
    )
    src = tmp_path / "tagged.wav"
    src.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
    with pytest.raises(signer.UnsupportedAssetError, match="16-bit PCM WAV"):
        signer.detect_format(src)
