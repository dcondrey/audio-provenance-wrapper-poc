"""Licence gating is a refusal, not a filter. A corpus that quietly shrinks is a silent result."""

import json

import pytest

from genomark_n.data.licences import (
    LicenceError,
    normalise_licence,
    require_allowed_licence,
    require_allowed_source,
)
from genomark_n.data.manifest import load_manifest


@pytest.mark.parametrize("licence", ["CC BY-NC 4.0", "CC BY-ND 4.0", "GPL-3.0", "unknown", ""])
def test_non_commercial_and_undeclared_licences_are_refused(licence):
    with pytest.raises(LicenceError):
        require_allowed_licence(licence, "test")


@pytest.mark.parametrize("licence", ["CC0", "CC BY 4.0", "cc-by-sa-4.0", "audio-provenance-owned"])
def test_commercial_licences_pass(licence):
    assert require_allowed_licence(licence, "test") in {
        "cc0-1.0", "cc-by-4.0", "cc-by-sa-4.0", "audio-provenance-owned"
    }


@pytest.mark.parametrize("source", ["fma", "openslr-28", "aachen-air", "deepawr", "dear",
                                    "silentcipher-weights", "timbre-watermarking"])
def test_denied_sources_are_refused_with_a_reason(source):
    with pytest.raises(LicenceError) as raised:
        require_allowed_source(source, "test")
    assert len(str(raised.value)) > 60


def test_manifest_rejects_an_nc_track(tmp_path):
    path = tmp_path / "manifest.jsonl"
    (tmp_path / "a.wav").write_bytes(b"")
    path.write_text(
        json.dumps({"path": "a.wav", "licence": "CC BY-NC-SA 4.0", "source": "jamendo"}) + "\n"
    )
    with pytest.raises(LicenceError):
        load_manifest(path)


def test_normalisation_is_stable():
    assert normalise_licence(" CC BY 4.0 ") == "cc-by-4.0"
    assert normalise_licence("CC0") == "cc0-1.0"
