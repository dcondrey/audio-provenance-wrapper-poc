from __future__ import annotations


import hashlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Sequence

from daemon.common import APW_VERSION

PLUGIN_NAME = "audio-provenance-wrapper"
PLUGIN_VERSION = APW_VERSION

PROOF_LEVELS = frozenset(
    {
        "directly_observed",
        "inferred",
        "user_declared",
        "externally_verified",
        "unknown_unobserved",
    }
)

RELATIONSHIPS = frozenset({"componentOf", "inputTo", "parentOf"})

UNOBSERVED_ASSERTION_LABEL = "apw.unobserved"
ACTIONS_ASSERTION_LABEL = "c2pa.actions.v2"

_DIGITAL_SOURCE_TYPE_BASE = "http://cv.iptc.org/newscodes/digitalsourcetype/"
DIGITAL_CAPTURE = _DIGITAL_SOURCE_TYPE_BASE + "digitalCapture"
COMPOSITE_CAPTURE = _DIGITAL_SOURCE_TYPE_BASE + "compositeCapture"
ALGORITHMICALLY_ENHANCED = _DIGITAL_SOURCE_TYPE_BASE + "algorithmicallyEnhanced"
MINOR_HUMAN_EDITS = _DIGITAL_SOURCE_TYPE_BASE + "minorHumanEdits"

_EDIT_ACTIONS: dict[str, tuple[str, str]] = {
    "clip_paste": ("c2pa.placed", COMPOSITE_CAPTURE),
    "clip_delete": ("c2pa.removed", MINOR_HUMAN_EDITS),
    "effect_change": ("c2pa.edited", ALGORITHMICALLY_ENHANCED),
    "sample_import_confirmed": ("c2pa.placed", COMPOSITE_CAPTURE),
    "arrangement_edit": ("c2pa.edited", MINOR_HUMAN_EDITS),
    "undo": ("c2pa.edited", MINOR_HUMAN_EDITS),
    "recorded": ("c2pa.created", DIGITAL_CAPTURE),
}

_UNKNOWN_ACTION = ("c2pa.unknown", MINOR_HUMAN_EDITS)


class ManifestError(ValueError):
    pass


def action_for_edit_type(edit_type: str) -> tuple[str, str]:
    return _EDIT_ACTIONS.get(edit_type, _UNKNOWN_ACTION)


def _software_agent() -> dict[str, str]:
    return {"name": PLUGIN_NAME, "version": PLUGIN_VERSION}


def _require_proof_level(value: str) -> str:
    if value not in PROOF_LEVELS:
        raise ManifestError(
            f"Unknown proof level {value!r}; expected one of {sorted(PROOF_LEVELS)}"
        )
    return value


def _slug(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in "-_." else "-" for ch in value).strip("-")
    if not cleaned:
        raise ManifestError("Ingredient titles must contain at least one usable character")
    return cleaned


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


DIGEST_KINDS: dict[str, str] = {
    "file_sha256": "sha-256 over the referenced file's bytes",
    "hash_chain_root": (
        "rolling hash-chain root over the observed routed windows; hashing the "
        "referenced file will not reproduce it"
    ),
}


@dataclass(frozen=True)
class Ingredient:
    title: str
    relationship: str
    proof_level: str
    sha256: str | None = None
    source_path: Path | None = None
    mime: str = "audio/wav"
    ingredient_id: str | None = None
    digest_kind: str = "file_sha256"

    def __post_init__(self) -> None:
        if self.digest_kind not in DIGEST_KINDS:
            raise ManifestError(
                f"Unknown digest kind {self.digest_kind!r}; expected one of {sorted(DIGEST_KINDS)}"
            )
        if self.relationship not in RELATIONSHIPS:
            raise ManifestError(
                f"Unknown ingredient relationship {self.relationship!r}; "
                f"expected one of {sorted(RELATIONSHIPS)}"
            )
        _require_proof_level(self.proof_level)
        digest = self.sha256
        if digest is None and self.source_path is not None:
            digest = sha256_file(self.source_path)
            object.__setattr__(self, "sha256", digest)
        if not digest:
            raise ManifestError(
                f"Ingredient {self.title!r} has neither a sha256 nor a readable source "
                "path; an ingredient without provenance must still record its hash"
            )
        if self.ingredient_id is None:
            object.__setattr__(self, "ingredient_id", _slug(self.title))

    def to_dict(self) -> dict[str, object]:
        # IMPORTANT: the field is named for what the value is. Writing a
        # hash-chain root under "apw:sha256" invites a registrar to hash the
        # stem file, find no match, and conclude the manifest is wrong.
        digest_field = "apw:sha256" if self.digest_kind == "file_sha256" else "apw:hash_chain_root"
        return {
            "title": self.title,
            "label": self.ingredient_id,
            "format": self.mime,
            "relationship": self.relationship,
            "metadata": {
                "apw:proof_level": self.proof_level,
                digest_field: self.sha256,
                "apw:digest_meaning": DIGEST_KINDS[self.digest_kind],
            },
        }


def unobserved_ingredient(title: str, source_path: Path, mime: str = "audio/wav") -> Ingredient:
    return Ingredient(
        title=title,
        relationship="inputTo",
        proof_level="unknown_unobserved",
        source_path=Path(source_path),
        mime=mime,
    )


_INGREDIENT_LINKED_ACTIONS = frozenset({"c2pa.placed", "c2pa.opened", "c2pa.removed"})

# The engine synthesizes these from ingredient relationships rather than from an
# observed edit, so they are labelled for what they are.
_LINEAGE_ACTION_PROOF_LEVEL = "inferred"
_CREATION_ACTION_PROOF_LEVEL = "unknown_unobserved"


@dataclass(frozen=True)
class ObservedAction:
    edit_type: str
    proof_level: str
    when: str | None = None
    confidence: float | None = None
    ingredient_ids: Sequence[str] = ()

    def to_dict(self) -> dict[str, object]:
        _require_proof_level(self.proof_level)
        action, source_type = action_for_edit_type(self.edit_type)
        if action in _INGREDIENT_LINKED_ACTIONS and not self.ingredient_ids:
            action = "c2pa.edited"
        entry: dict[str, object] = {
            "action": action,
            "digitalSourceType": source_type,
            "softwareAgent": _software_agent(),
        }
        if self.when is not None:
            entry["when"] = self.when
        entry["parameters"] = {
            "apw:edit_type": self.edit_type,
            "apw:proof_level": self.proof_level,
        }
        if self.ingredient_ids:
            entry["parameters"]["ingredientIds"] = list(self.ingredient_ids)
        if self.confidence is not None:
            entry["parameters"]["apw:confidence"] = self.confidence
        return entry


@dataclass(frozen=True)
class UnobservedClaim:
    claim: str
    value: object
    evidence: str
    proof_level: str = "unknown_unobserved"

    def to_dict(self) -> dict[str, object]:
        return {
            "claim": self.claim,
            "value": self.value,
            "evidence": self.evidence,
            "apw:proof_level": _require_proof_level(self.proof_level),
        }


FULL_DAW_PROVENANCE_UNOBSERVED = UnobservedClaim(
    claim="full_daw_provenance",
    value=False,
    evidence="Only audio routed through the capture plugin was observed",
)


@dataclass
class ManifestSpec:
    title: str
    actions: Sequence[ObservedAction] = field(default_factory=tuple)
    ingredients: Sequence[Ingredient] = field(default_factory=tuple)
    unobserved: Sequence[UnobservedClaim] = field(default_factory=tuple)
    extra_assertions: Sequence[dict[str, object]] = field(default_factory=tuple)
    mime: str = "audio/wav"


def _ingredient_actions(
    ingredients: Sequence[Ingredient], already_referenced: frozenset[str]
) -> list[dict[str, object]]:
    """Link ingredients to the actions that consumed them.

    REQUIRED: claim v2 rejects a c2pa.placed action that references anything but a
    componentOf ingredient (assertion.action.ingredientMismatch). parentOf ingredients are
    the lineage of the leading c2pa.opened action; inputTo ingredients stay unreferenced.
    """
    ids = [
        ing.ingredient_id
        for ing in ingredients
        if ing.relationship == "componentOf" and ing.ingredient_id not in already_referenced
    ]
    if not ids:
        return []
    return [
        {
            "action": "c2pa.placed",
            "digitalSourceType": COMPOSITE_CAPTURE,
            "softwareAgent": _software_agent(),
            "parameters": {
                "ingredientIds": ids,
                "apw:proof_level": _LINEAGE_ACTION_PROOF_LEVEL,
                "apw:basis": (
                    "synthesized from the ingredient's recorded relationship to the export, "
                    "not from an observed placement edit"
                ),
            },
        }
    ]


def build_manifest(spec: ManifestSpec) -> dict[str, object]:
    unobserved = list(spec.unobserved)
    if not any(item.claim == FULL_DAW_PROVENANCE_UNOBSERVED.claim for item in unobserved):
        unobserved.insert(0, FULL_DAW_PROVENANCE_UNOBSERVED)

    parent_ids = [
        ing.ingredient_id for ing in spec.ingredients if ing.relationship == "parentOf"
    ]
    if parent_ids:
        actions: list[dict[str, object]] = [
            {
                "action": "c2pa.opened",
                "digitalSourceType": COMPOSITE_CAPTURE,
                "softwareAgent": _software_agent(),
                "parameters": {
                    "ingredientIds": parent_ids,
                    "apw:proof_level": _LINEAGE_ACTION_PROOF_LEVEL,
                },
            }
        ]
    else:
        # IMPORTANT: never digitalCapture. That asserts origin at a capture
        # device, the strongest provenance statement there is, and this engine
        # never observes the creation act; a master bounced from purchased loops
        # would carry it too. compositeCapture is what the evidence supports.
        actions = [
            {
                "action": "c2pa.created",
                "digitalSourceType": COMPOSITE_CAPTURE,
                "softwareAgent": _software_agent(),
                "parameters": {
                    "apw:proof_level": _CREATION_ACTION_PROOF_LEVEL,
                    "apw:basis": (
                        "the creation act itself was not observed; only routed audio and the "
                        "exported file were"
                    ),
                },
            }
        ]
    actions.extend(action.to_dict() for action in spec.actions)

    referenced = frozenset(parent_ids).union(
        ingredient_id for action in spec.actions for ingredient_id in action.ingredient_ids
    )
    known_ids: set[str] = set()
    for ingredient in spec.ingredients:
        if ingredient.ingredient_id in known_ids:
            raise ManifestError(
                f"Duplicate ingredient id {ingredient.ingredient_id!r}; give the colliding "
                "ingredient an explicit ingredient_id so actions reference exactly one file"
            )
        known_ids.add(ingredient.ingredient_id)
    unknown = referenced - known_ids
    if unknown:
        raise ManifestError(f"Actions reference unknown ingredient ids: {sorted(unknown)}")
    actions.extend(_ingredient_actions(spec.ingredients, referenced))

    assertions: list[dict[str, object]] = [
        {"label": ACTIONS_ASSERTION_LABEL, "data": {"actions": actions}},
        {
            "label": UNOBSERVED_ASSERTION_LABEL,
            "data": {"items": [item.to_dict() for item in unobserved]},
        },
    ]
    for extra in spec.extra_assertions:
        label = extra.get("label")
        if not isinstance(label, str) or "data" not in extra:
            raise ManifestError("Extra assertions require a string label and a data field")
        if label in {ACTIONS_ASSERTION_LABEL, UNOBSERVED_ASSERTION_LABEL}:
            raise ManifestError(f"Assertion {label} is owned by the engine and cannot be overridden")
        assertions.append({"label": label, "data": extra["data"]})

    manifest: dict[str, object] = {
        "title": spec.title,
        "format": spec.mime,
        "claim_generator_info": [
            {"name": PLUGIN_NAME, "version": PLUGIN_VERSION}
        ],
        "assertions": assertions,
    }
    if spec.ingredients:
        manifest["ingredients"] = [ing.to_dict() for ing in spec.ingredients]
    return manifest


def assertion_labels(manifest: dict[str, object]) -> list[str]:
    assertions = manifest.get("assertions")
    if not isinstance(assertions, list):
        return []
    return [a["label"] for a in assertions if isinstance(a, dict) and isinstance(a.get("label"), str)]
