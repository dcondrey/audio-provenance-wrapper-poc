"""The JSON Schema subset docs/manifest.schema.json uses, and nothing else.

The repository carries no jsonschema dependency. rust/apw-core/tests/
time_anchor_parity.rs implements the same subset, so a keyword this file does not
understand fails loudly in both places instead of being skipped.
"""
from __future__ import annotations

import re

_SUPPORTED = {
    "$schema", "$id", "$defs", "$ref", "title", "description", "type", "required",
    "properties", "allOf", "enum", "const", "items", "pattern", "minLength", "minimum",
    "maximum", "not", "additionalProperties",
}
_TYPES = {
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
    "string": lambda v: isinstance(v, str),
    "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "boolean": lambda v: isinstance(v, bool),
    "null": lambda v: v is None,
}


def validate(instance: object, schema: dict, root: dict | None = None, path: str = "") -> list[str]:
    root = root if root is not None else schema
    errors: list[str] = []
    unknown = set(schema) - _SUPPORTED
    if unknown:
        raise ValueError(f"unsupported schema keyword(s) at {path or '/'}: {sorted(unknown)}")
    if "$ref" in schema:
        prefix = "#/$defs/"
        ref = schema["$ref"]
        if not ref.startswith(prefix):
            raise ValueError(f"unsupported $ref {ref}")
        errors += validate(instance, root["$defs"][ref[len(prefix):]], root, path)
    for sub in schema.get("allOf", []):
        errors += validate(instance, sub, root, path)
    if "type" in schema:
        wanted = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(_TYPES[name](instance) for name in wanted):
            errors.append(f"{path or '/'}: expected type {wanted}")
    if "const" in schema and not _same(instance, schema["const"]):
        errors.append(f"{path or '/'}: expected const {schema['const']!r}")
    if "enum" in schema and not any(_same(instance, option) for option in schema["enum"]):
        errors.append(f"{path or '/'}: {instance!r} is not one of {schema['enum']}")
    if "not" in schema and not validate(instance, schema["not"], root, path):
        errors.append(f"{path or '/'}: matches a forbidden schema")
    if isinstance(instance, str):
        if "minLength" in schema and len(instance) < schema["minLength"]:
            errors.append(f"{path or '/'}: shorter than {schema['minLength']}")
        if "pattern" in schema and re.search(schema["pattern"], instance) is None:
            errors.append(f"{path or '/'}: does not match {schema['pattern']}")
    if isinstance(instance, (int, float)) and not isinstance(instance, bool):
        if "minimum" in schema and instance < schema["minimum"]:
            errors.append(f"{path or '/'}: below {schema['minimum']}")
        if "maximum" in schema and instance > schema["maximum"]:
            errors.append(f"{path or '/'}: above {schema['maximum']}")
    if isinstance(instance, dict):
        for key in schema.get("required", []):
            if key not in instance:
                errors.append(f"{path}/{key}: required")
        for key, sub in schema.get("properties", {}).items():
            if key in instance:
                errors += validate(instance[key], sub, root, f"{path}/{key}")
        extra = schema.get("additionalProperties")
        if extra is False:
            declared = set(schema.get("properties", {}))
            errors += [f"{path}/{key}: not allowed" for key in instance if key not in declared]
    if isinstance(instance, list) and "items" in schema:
        for index, item in enumerate(instance):
            errors += validate(item, schema["items"], root, f"{path}/{index}")
    return errors


def _same(left: object, right: object) -> bool:
    return type(left) is type(right) and left == right
