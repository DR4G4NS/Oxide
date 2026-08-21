"""JAR identity checks shared by gates and certification self-tests.

The current-target resolver uses this module as the final, local authority for
an artifact.  In particular, a cache entry is never trusted merely because it
exists: its size, digest, ZIP structure, ``version.properties`` and build are
checked before it is returned to a caller.
"""

from __future__ import annotations

import hashlib
import re
import zipfile
import zlib
from dataclasses import dataclass
from pathlib import Path


class JarIdentityError(ValueError):
    """Raised when a file is not the exact JAR described by current.toml."""


@dataclass(frozen=True)
class JarIdentity:
    """Measured identity of a validated JAR."""

    size_bytes: int
    sha256: str
    build: str


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read_jar_build(jar_path: Path) -> str:
    """Return ``build=`` from ``version.properties`` or fail closed."""
    try:
        with zipfile.ZipFile(jar_path) as z:
            names = set(z.namelist())
            if "version.properties" not in names:
                raise JarIdentityError(f"version.properties missing in {jar_path}")
            # Read only the authoritative member.  Calling testzip() scans
            # every member and lets unrelated ZIP bombs dominate certification.
            with z.open("version.properties") as version_stream:
                props = version_stream.read().decode("utf-8")
    except JarIdentityError:
        raise
    except (
        OSError,
        UnicodeDecodeError,
        zipfile.BadZipFile,
        zipfile.LargeZipFile,
        NotImplementedError,
        RuntimeError,
        zlib.error,
    ) as exc:
        raise JarIdentityError(f"invalid JAR/ZIP {jar_path}: {exc}") from exc
    for line in props.splitlines():
        stripped = line.strip()
        match = re.fullmatch(r"build\s*=\s*(.+?)\s*", stripped)
        if match:
            return match.group(1)
    raise JarIdentityError(f"build missing in version.properties: {jar_path}")


def check_jar_build(jar_path: Path, expected_build: str) -> str | None:
    """Return an error message if JAR build mismatches; None on success."""
    try:
        reported = read_jar_build(jar_path)
    except JarIdentityError as exc:
        return str(exc)
    if reported != expected_build:
        return f"JAR build mismatch: requested {expected_build}, but JAR reports {reported}"
    return None


def check_jar_sha256(jar_path: Path, expected_sha: str) -> str | None:
    """Return an error message if JAR SHA256 mismatches; None on success."""
    got = sha256_file(jar_path)
    if got != expected_sha:
        return f"JAR SHA mismatch: {got} != {expected_sha}"
    return None


def validate_jar_identity(
    jar_path: Path,
    *,
    expected_build: str,
    expected_size_bytes: int,
    expected_sha256: str,
) -> JarIdentity:
    """Validate and return a JAR's measured identity.

    Validation is deliberately ordered so a truncated/error response fails on
    size before an expensive digest, while the ZIP and version checks reject
    HTML downloads and unrelated archives even when a caller supplies a stale
    expected digest.  ``JarIdentityError`` is the single fail-closed error
    type consumed by the release resolver and its hermetic tests.
    """

    path = Path(jar_path)
    if not path.exists() or not path.is_file():
        raise JarIdentityError(f"JAR missing: {path}")
    try:
        size = path.stat().st_size
    except OSError as exc:
        raise JarIdentityError(f"cannot stat JAR {path}: {exc}") from exc
    if size != expected_size_bytes:
        raise JarIdentityError(
            f"JAR size mismatch: {size} != {expected_size_bytes} ({path})"
        )
    try:
        digest = sha256_file(path)
    except OSError as exc:
        raise JarIdentityError(f"cannot read JAR {path}: {exc}") from exc
    if digest.lower() != expected_sha256.lower():
        raise JarIdentityError(f"JAR SHA mismatch: {digest} != {expected_sha256}")
    try:
        build = read_jar_build(path)
    except JarIdentityError:
        raise
    if build != expected_build:
        raise JarIdentityError(
            f"JAR build mismatch: requested {expected_build}, but JAR reports {build}"
        )
    return JarIdentity(size_bytes=size, sha256=digest, build=build)
