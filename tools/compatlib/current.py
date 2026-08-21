"""Read compat/current.toml without embedding machine-local paths."""

from __future__ import annotations

import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent


def load_current(path: Path | None = None) -> dict:
    target = path or (REPO_ROOT / "compat" / "current.toml")
    with target.open("rb") as f:
        return tomllib.load(f)


def current_build(data: dict | None = None) -> str:
    doc = data if data is not None else load_current()
    return str(doc["target"]["build"])


def current_source_tag(data: dict | None = None) -> str:
    doc = data if data is not None else load_current()
    return str(doc["target"]["source_tag"])


def current_source_commit(data: dict | None = None) -> str:
    doc = data if data is not None else load_current()
    return str(doc["target"]["source_commit"])


def current_jar_sha256(data: dict | None = None) -> str:
    doc = data if data is not None else load_current()
    return str(doc["target"]["jar_sha256"])
