"""Licence gating, enforced in code because spec 9.1/9.2 make it gating rather than advisory.

A track or impulse response whose declared licence is not in ALLOWED_LICENCES is REFUSED, loudly.
Silently skipping it would let a corpus quietly shrink to nothing and a run report a result over
data nobody checked.
"""

ALLOWED_LICENCES: frozenset[str] = frozenset(
    {
        "cc0-1.0",
        "cc-by-3.0",
        "cc-by-4.0",
        "cc-by-sa-3.0",
        "cc-by-sa-4.0",
        "audio-provenance-owned",
        "audio-provenance-licensed",
        "echothief-convolution-grant",
        "public-domain",
    }
)

DENIED_SOURCES: dict[str, str] = {
    "fma": (
        "spec 9.1: FMA metadata is CC BY-NC-SA 4.0 and its audio carries heterogeneous per-track "
        "licences, a substantial fraction NonCommercial. Usable only through a per-track filtered "
        "manifest, which must then declare each track's own licence and will not carry this source id."
    ),
    "openslr-28": (
        "spec 9.2: RIRS_NOISES is 16 kHz only. Driving a 48 kHz layer with it fabricates the entire "
        "8-24 kHz band, which is precisely where transducer roll-off lives."
    ),
    "rirs_noises": "alias of openslr-28; see that entry.",
    "aachen-air": "spec 9.2: research-use only, notwithstanding that AWARE used it.",
    "silentcipher-weights": (
        "The GitHub release tarball contains no LICENSE/NOTICE/COPYING and the repository README "
        "scopes its MIT grant to 'the code in this repository'. The HuggingFace mirror has "
        "cardData: null. The weights carry no grant."
    ),
    "deepawr": (
        "Declared MIT, but 26%/43%/39% of substantive lines in train_audio.py, extract.py and "
        "embed.py are verbatim from DeAR, an unlicensed drop, and the author sets do not overlap. "
        "Reference document only: never a dependency, never vendored source or weights."
    ),
    "dear": "Unlicensed Google Drive drop, confirmed by enumeration. Not vendorable.",
    "timbre-watermarking": "GPL-3.0. Would force Audio Provenance's source open.",
}

_ALIASES = {
    "cc0": "cc0-1.0",
    "cc-0": "cc0-1.0",
    "cc0 1.0": "cc0-1.0",
    "cc by 4.0": "cc-by-4.0",
    "cc-by": "cc-by-4.0",
    "ccby4.0": "cc-by-4.0",
    "cc by-sa 4.0": "cc-by-sa-4.0",
    "cc-by-sa": "cc-by-sa-4.0",
    "cc by 3.0": "cc-by-3.0",
    "pd": "public-domain",
}


class LicenceError(ValueError):
    """A corpus item's licence does not permit commercial training use."""


def normalise_licence(licence: str) -> str:
    key = " ".join(str(licence).strip().lower().split())
    key = _ALIASES.get(key, key)
    return _ALIASES.get(key.replace(" ", "-"), key.replace(" ", "-"))


def require_allowed_licence(licence: str, where: str) -> str:
    normalised = normalise_licence(licence)
    if normalised not in ALLOWED_LICENCES:
        raise LicenceError(
            f"{where}: licence {licence!r} (normalised {normalised!r}) is not in the commercial "
            f"allowlist {sorted(ALLOWED_LICENCES)}. Spec 9.1/9.2 make this gating: NC and ND terms "
            "are excluded, and an undeclared licence is treated as no grant."
        )
    return normalised


def require_allowed_source(source: str, where: str) -> str:
    key = normalise_licence(source)
    if key in DENIED_SOURCES:
        raise LicenceError(f"{where}: source {source!r} is denied. {DENIED_SOURCES[key]}")
    return key
