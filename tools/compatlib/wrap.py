"""Provenance wrappers for compatibility artifacts."""

from __future__ import annotations

from . import GENERATOR_NAME, GENERATOR_VERSION, SCHEMA_VERSION


def provenance(
    *,
    build: str,
    source_ref: str,
    source_commit: str,
    jar_filename: str | None = None,
    jar_size: int | None = None,
    jar_sha256: str | None = None,
) -> dict:
    doc = {
        "schema_version": SCHEMA_VERSION,
        "build": build,
        "source_ref": source_ref,
        "source_commit": source_commit,
        "generator": {"name": GENERATOR_NAME, "version": GENERATOR_VERSION},
    }
    if jar_sha256 is not None:
        doc["jar"] = {
            "filename": jar_filename or "desktop.jar",
            "size_bytes": jar_size,
            "sha256": jar_sha256,
        }
    return doc


def wrap(payload_key: str, payload, **kwargs) -> dict:
    doc = provenance(**kwargs)
    doc[payload_key] = payload
    return doc
