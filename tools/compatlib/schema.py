"""Validate compatibility artifact schema v2 and provenance."""

from __future__ import annotations

import json
import re
from pathlib import Path

from . import SCHEMA_VERSION

ABS_PATH_RE = re.compile(r"(^|/)(home|Users|opt|tmp)/|/home/|/Users/")
REQUIRED_PROVENANCE = (
    "schema_version",
    "build",
    "source_ref",
    "source_commit",
    "generator",
)

ARTIFACT_KEYS = {
    "manifest.json": None,
    "packets.json": "packets",
    "streams.json": "streams",
    "rpc.json": "rpc",
    "typeio.json": "typeio",
    "saves.json": "saves",
    "content.json": "content",
    "entities.json": "entities",
    "entity-sync.json": "entity_sync",
    "rules.json": "rules",
    "logic.json": "logic",
    "semantic-fingerprints.json": "fingerprints",
}


class SchemaError(ValueError):
    pass


def load_json(path: Path) -> dict | list:
    return json.loads(path.read_text(encoding="utf-8"))


def contains_local_path(obj) -> list[str]:
    hits = []

    def walk(value, prefix=""):
        if isinstance(value, dict):
            for k, v in value.items():
                if k in {"path", "jar_path", "absolute_path", "local_path"}:
                    if isinstance(v, str) and v.startswith("/"):
                        hits.append(f"{prefix}{k}={v}")
                walk(v, prefix + k + ".")
        elif isinstance(value, list):
            for i, v in enumerate(value):
                walk(v, prefix + f"[{i}].")
        elif isinstance(value, str):
            if ABS_PATH_RE.search(value) and value.startswith("/"):
                hits.append(f"{prefix}{value}")

    walk(obj)
    return hits


def validate_provenance(doc: dict, *, require_jar: bool = False) -> list[str]:
    errors = []
    for key in REQUIRED_PROVENANCE:
        if key not in doc:
            errors.append(f"missing provenance key {key}")
    if doc.get("schema_version") != SCHEMA_VERSION:
        errors.append(
            f"schema_version {doc.get('schema_version')!r} != {SCHEMA_VERSION}"
        )
    gen = doc.get("generator")
    if not isinstance(gen, dict) or "name" not in gen or "version" not in gen:
        errors.append("generator must be {name, version}")
    if require_jar or "jar" in doc:
        jar = doc.get("jar")
        if not isinstance(jar, dict):
            errors.append("jar must be an object")
        else:
            for k in ("filename", "size_bytes", "sha256"):
                if k not in jar:
                    errors.append(f"jar missing {k}")
            if "path" in jar:
                errors.append("jar.path is not portable; use filename/size/sha256")
    errors.extend(f"local path leaked: {h}" for h in contains_local_path(doc))
    return errors


def validate_artifact_file(path: Path, expected_build: str | None = None) -> list[str]:
    errors = []
    try:
        doc = load_json(path)
    except Exception as exc:
        return [f"{path}: invalid JSON: {exc}"]
    if not isinstance(doc, dict):
        return [f"{path}: artifact must be a provenance-wrapped object, not {type(doc).__name__}"]
    errors.extend(f"{path.name}: {e}" for e in validate_provenance(doc, require_jar=path.name == "manifest.json"))
    if expected_build and doc.get("build") != expected_build:
        errors.append(f"{path.name}: build {doc.get('build')!r} != {expected_build!r}")
    payload_key = ARTIFACT_KEYS.get(path.name)
    if payload_key and payload_key not in doc and path.name != "manifest.json":
        errors.append(f"{path.name}: missing payload key {payload_key}")
    return errors


def validate_compat_dir(compat_dir: Path, expected_build: str | None = None) -> list[str]:
    errors = []
    if not compat_dir.is_dir():
        return [f"missing directory {compat_dir}"]
    present = {p.name for p in compat_dir.glob("*.json")}
    required = {"manifest.json", "packets.json", "saves.json", "content.json", "rules.json", "logic.json"}
    for name in sorted(required):
        if name not in present:
            errors.append(f"missing required artifact {name}")
    for path in sorted(compat_dir.glob("*.json")):
        if path.name == "certification-ledger.json":
            continue
        errors.extend(validate_artifact_file(path, expected_build=expected_build or compat_dir.name))
    return errors
