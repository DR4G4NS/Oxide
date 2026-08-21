"""Deterministic, atomic JSON artifact writes."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path


def canonical_dumps(obj) -> str:
    return json.dumps(obj, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def atomic_write_tree(dest_dir: Path, files: dict[str, object]) -> None:
    """Write a complete artifact tree, replacing dest_dir only after all files succeed."""
    dest_dir.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=str(dest_dir.parent)) as tmp:
        tmp_path = Path(tmp)
        staged = tmp_path / dest_dir.name
        staged.mkdir()
        for name, obj in files.items():
            out = staged / name
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(canonical_dumps(obj), encoding="utf-8")
        backup = None
        if dest_dir.exists():
            backup = dest_dir.with_name(dest_dir.name + ".bak")
            if backup.exists():
                _rmtree(backup)
            dest_dir.rename(backup)
        staged.rename(dest_dir)
        if backup is not None:
            _rmtree(backup)


def _rmtree(path: Path) -> None:
    for child in sorted(path.rglob("*"), reverse=True):
        if child.is_file() or child.is_symlink():
            child.unlink()
        elif child.is_dir():
            child.rmdir()
    path.rmdir()
