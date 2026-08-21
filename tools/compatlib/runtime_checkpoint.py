"""Stable CERTIFIED_RUNTIME_SHA checkpoint (non-self-referential).

Committed metadata records a runtime-implementation checkpoint. Later
docs/test/certification-only descendants may exist without changing runtime.
Exact CI HEAD is recorded externally (PR body / Actions / release metadata),
not required to equal the committed checkpoint SHA.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

# Paths whose post-checkpoint mutation is treated as runtime drift.
# Keep conservative: compiled code, Cargo graph, and compile-time target SSoT.
RUNTIME_AFFECTING_PREFIXES = (
    "src/",
)
RUNTIME_AFFECTING_EXACT = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "compat/current.toml",
    }
)

_HEX_SHA = re.compile(r"^[0-9a-fA-F]+$")
_MIN_HEX_LEN = 7  # git accepts abbreviated SHAs; require at least this many hex chars


class RuntimeCheckpointError(ValueError):
    """Fail-closed certification checkpoint violation."""


def is_runtime_affecting_path(path: str) -> bool:
    """Return True if a repo-relative path is runtime-affecting."""
    normalized = path.replace("\\", "/").lstrip("./")
    if normalized in RUNTIME_AFFECTING_EXACT:
        return True
    for prefix in RUNTIME_AFFECTING_PREFIXES:
        if normalized == prefix.rstrip("/") or normalized.startswith(prefix):
            return True
    return False


def _git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        check=check,
    )


def resolve_commit(repo: Path, rev: str) -> str:
    """Resolve rev to a full commit SHA, or raise RuntimeCheckpointError."""
    text = (rev or "").strip()
    if not text:
        raise RuntimeCheckpointError(f"CERTIFIED_RUNTIME_SHA missing: {rev!r}")
    # Hex SHAs must be long enough; symbolic refs (HEAD, branches) are allowed.
    if _HEX_SHA.fullmatch(text) and len(text) < _MIN_HEX_LEN:
        raise RuntimeCheckpointError(
            f"CERTIFIED_RUNTIME_SHA missing or too short: {rev!r}"
        )
    res = _git(repo, "rev-parse", "--verify", f"{text}^{{commit}}", check=False)
    if res.returncode != 0:
        raise RuntimeCheckpointError(
            f"CERTIFIED_RUNTIME_SHA does not resolve to a commit: {text}"
        )
    return res.stdout.strip()


def is_ancestor(repo: Path, ancestor: str, descendant: str = "HEAD") -> bool:
    res = _git(
        repo,
        "merge-base",
        "--is-ancestor",
        ancestor,
        descendant,
        check=False,
    )
    return res.returncode == 0


def changed_paths_since(repo: Path, checkpoint: str, head: str = "HEAD") -> list[str]:
    """List paths changed in (checkpoint, head] (exclusive of checkpoint)."""
    res = _git(
        repo,
        "diff",
        "--name-only",
        f"{checkpoint}..{head}",
        check=False,
    )
    if res.returncode != 0:
        raise RuntimeCheckpointError(
            f"git diff failed for {checkpoint}..{head}: {res.stderr.strip()}"
        )
    return [line.strip() for line in res.stdout.splitlines() if line.strip()]


def runtime_drift_paths(paths: list[str]) -> list[str]:
    return sorted({p for p in paths if is_runtime_affecting_path(p)})


def load_certified_runtime_sha(ledger: dict) -> str:
    """Read CERTIFIED_RUNTIME_SHA from ledger; reject stale CERTIFIED_CODE_SHA."""
    if "certified_code_sha" in ledger:
        raise RuntimeCheckpointError(
            "stale field certified_code_sha present; use certified_runtime_sha"
        )
    sha = ledger.get("certified_runtime_sha")
    if not isinstance(sha, str) or not sha.strip():
        raise RuntimeCheckpointError(
            "CERTIFIED_RUNTIME_SHA missing: ledger requires certified_runtime_sha"
        )
    return sha.strip()


def validate_runtime_checkpoint(
    repo: Path,
    certified_runtime_sha: str,
    *,
    head: str = "HEAD",
) -> list[str]:
    """Return error strings (empty = OK). Fail-closed for drift / ancestry."""
    errors: list[str] = []
    try:
        checkpoint = resolve_commit(repo, certified_runtime_sha)
        head_sha = resolve_commit(repo, head)
    except RuntimeCheckpointError as exc:
        return [str(exc)]

    if not is_ancestor(repo, checkpoint, head_sha):
        errors.append(
            f"CERTIFIED_RUNTIME_SHA {checkpoint} is not an ancestor of {head_sha}"
        )
        return errors

    changed = changed_paths_since(repo, checkpoint, head_sha)
    drift = runtime_drift_paths(changed)
    if drift:
        preview = ", ".join(drift[:12])
        more = f" (+{len(drift) - 12} more)" if len(drift) > 12 else ""
        errors.append(
            "runtime drift after CERTIFIED_RUNTIME_SHA: "
            f"{preview}{more}"
        )
    return errors


def validate_ledger_runtime_checkpoint(repo: Path, ledger: dict) -> list[str]:
    """Load SHA from ledger and validate against repo HEAD."""
    try:
        sha = load_certified_runtime_sha(ledger)
    except RuntimeCheckpointError as exc:
        return [str(exc)]
    return validate_runtime_checkpoint(repo, sha)
