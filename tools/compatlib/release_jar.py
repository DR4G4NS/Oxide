"""Resolve and verify an official Mindustry release JAR.

This module intentionally keeps GitHub transport separate from artifact
validation.  The latter is deterministic and hermetic, which lets
``compat_selftest.py`` exercise all failure modes without downloading the
82-MB current target on every run.

The values in ``compat/current.toml`` are authoritative.  GitHub metadata is
used to validate the exact source tag/commit; the binary is discovered from
the official itch.io Linux distribution because its embedded desktop JAR is
the target artifact.  A downloaded or cached file is accepted only after its
size, SHA-256, ZIP structure, ``version.properties`` and build have all been
checked.
"""

from __future__ import annotations

import argparse
import http.client
from html.parser import HTMLParser
import json
import os
import re
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
import zlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping

from .current import load_current
from .jar_identity import JarIdentity, JarIdentityError, validate_jar_identity

DEFAULT_REPOSITORY = "Anuken/Mindustry"
GITHUB_API = "https://api.github.com"
GITHUB_API_HOSTS = ("api.github.com",)
ITCH_GAME_URL = "https://anuke.itch.io/mindustry"
ITCH_LANDING_HOSTS = ("anuke.itch.io",)
ITCH_CDN_SUFFIXES = (".r2.cloudflarestorage.com",)
MAX_ITCH_ARCHIVE_BYTES = 220 * 1024 * 1024
MAX_NEGOTIATION_RESPONSE_BYTES = 4 * 1024 * 1024
_SHA256 = re.compile(r"^[0-9a-fA-F]{64}$")


class ReleaseResolutionError(RuntimeError):
    """Raised for any release lookup, download or identity failure."""


@dataclass(frozen=True)
class CurrentTarget:
    build: str
    source_tag: str
    source_commit: str
    jar_filename: str
    jar_size_bytes: int
    jar_sha256: str

    @classmethod
    def from_current(cls, current: Mapping[str, Any]) -> "CurrentTarget":
        try:
            target = current["target"]
            values = {
                "build": str(target["build"]),
                "source_tag": str(target["source_tag"]),
                "source_commit": str(target["source_commit"]),
                "jar_filename": str(target["jar_filename"]),
                "jar_size_bytes": int(target["jar_size_bytes"]),
                "jar_sha256": str(target["jar_sha256"]).lower(),
            }
        except (KeyError, TypeError, ValueError) as exc:
            raise ReleaseResolutionError(
                "compat/current.toml is missing a required current target field"
            ) from exc
        if not values["build"] or not values["source_tag"] or not values["source_commit"]:
            raise ReleaseResolutionError("current target build/tag/commit must be non-empty")
        if not values["jar_filename"] or Path(values["jar_filename"]).name != values["jar_filename"]:
            raise ReleaseResolutionError("current target jar_filename must be a plain filename")
        if values["jar_size_bytes"] <= 0:
            raise ReleaseResolutionError("current target jar_size_bytes must be positive")
        if not _SHA256.fullmatch(values["jar_sha256"]):
            raise ReleaseResolutionError("current target jar_sha256 must be 64 hexadecimal characters")
        return cls(**values)


@dataclass(frozen=True)
class ReleaseAsset:
    name: str
    download_url: str
    size: int | None = None


@dataclass(frozen=True)
class ItchUpload:
    """One upload discovered from itch.io's signed download page.

    The upload id is deliberately discovered at runtime from the page.  It is
    not a release authority: itch can replace an upload while retaining the
    game page, so the resulting archive and extracted JAR are still checked
    against ``compat/current.toml``.
    """

    name: str
    version: str
    upload_id: str
    platforms: frozenset[str]


@dataclass(frozen=True)
class ItchDownload:
    """A signed archive URL selected from the exact Linux itch upload."""

    upload: ItchUpload
    landing_url: str
    archive_url: str


def target_from_file(path: Path) -> CurrentTarget:
    """Load and validate the current target from a TOML file."""

    return CurrentTarget.from_current(load_current(Path(path)))


def _urlopen(request: urllib.request.Request | str, timeout: int = 60):
    return urllib.request.urlopen(request, timeout=timeout)


class _ItchUploadParser(HTMLParser):
    """Extract upload rows without depending on BeautifulSoup or page JS."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self._stack: list[str] = []
        self._row: dict[str, Any] | None = None
        self._rows: list[dict[str, Any]] = []
        self._text_field: str | None = None
        self._text_depth = 0

    @staticmethod
    def _attrs(attrs: list[tuple[str, str | None]]) -> dict[str, str]:
        return {key: value or "" for key, value in attrs}

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = self._attrs(attrs)
        classes = set(values.get("class", "").split())
        if self._row is None and tag == "div" and "upload" in classes:
            self._row = {
                "name": "",
                "version": "",
                "upload_id": "",
                "platforms": set(),
            }
            self._stack = [tag]
            return
        if self._row is None:
            return

        self._stack.append(tag)
        if tag == "strong" and "name" in classes and values.get("title"):
            self._row["name"] = values["title"]
        if tag == "a" and "download_btn" in classes:
            self._row["upload_id"] = values.get("data-upload_id", "")
        if tag == "span":
            title = values.get("title", "").lower()
            if "download for linux" in title or "icon-tux" in classes:
                self._row["platforms"].add("linux")
            if "download for windows" in title or "icon-windows8" in classes:
                self._row["platforms"].add("windows")
            if "download for mac" in title or "icon-apple" in classes:
                self._row["platforms"].add("macos")
            if "download for android" in title or "icon-android" in classes:
                self._row["platforms"].add("android")
            if "version_name" in classes:
                self._text_field = "version"
                self._text_depth = len(self._stack)

    def handle_data(self, data: str) -> None:
        if self._row is not None and self._text_field == "version":
            self._row["version"] += data

    def handle_endtag(self, tag: str) -> None:
        if self._row is None:
            return
        if self._text_field == "version" and len(self._stack) <= self._text_depth:
            self._text_field = None
            self._text_depth = 0
        if self._stack:
            # HTMLParser reports the well-formed itch markup in nesting order;
            # tolerate omitted optional closing tags while retaining the row.
            try:
                index = len(self._stack) - 1 - self._stack[::-1].index(tag)
            except ValueError:
                index = -1
            if index >= 0:
                del self._stack[index:]
        if tag == "div" and not self._stack:
            self._row["version"] = " ".join(str(self._row["version"]).split())
            self._rows.append(self._row)
            self._row = None

    def rows(self) -> list[dict[str, Any]]:
        return self._rows


def parse_itch_uploads(page_html: str) -> tuple[ItchUpload, ...]:
    """Parse upload metadata from an itch signed-download HTML page."""

    parser = _ItchUploadParser()
    try:
        parser.feed(page_html)
        parser.close()
    except (ValueError, AssertionError) as exc:
        raise ReleaseResolutionError(f"cannot parse itch.io upload page: {exc}") from exc
    uploads: list[ItchUpload] = []
    for row in parser.rows():
        platforms = frozenset(str(value) for value in row["platforms"])
        uploads.append(
            ItchUpload(
                name=str(row["name"]),
                version=str(row["version"]),
                upload_id=str(row["upload_id"]),
                platforms=platforms,
            )
        )
    return tuple(uploads)


def select_itch_upload(page_html: str, target: CurrentTarget) -> ItchUpload:
    """Select exactly one Linux-64bit upload for the requested build.

    Selection is based on the upload's advertised filename, Linux platform
    marker and exact version label.  Upload ids are only transport handles
    discovered after this selection; no id is hard-coded in the resolver.
    """

    uploads = parse_itch_uploads(page_html)
    expected_name = "[Linux-64bit]Mindustry.zip"
    named = [upload for upload in uploads if upload.name == expected_name]
    if len(named) == 0:
        raise ReleaseResolutionError(
            f"itch.io Linux upload missing for build {target.build}: expected {expected_name}"
        )
    if len(named) != 1:
        raise ReleaseResolutionError(
            f"ambiguous itch.io Linux uploads for build {target.build}: {len(named)} matches"
        )
    upload = named[0]
    if upload.platforms != frozenset({"linux"}):
        raise ReleaseResolutionError(
            f"itch.io upload {upload.name!r} has wrong platform metadata: "
            f"{sorted(upload.platforms)!r}"
        )
    expected_version = f"Version {target.build}"
    if upload.version != expected_version:
        raise ReleaseResolutionError(
            f"itch.io upload version mismatch: {upload.version!r} != {expected_version!r}"
        )
    if not re.fullmatch(r"[0-9]+", upload.upload_id):
        raise ReleaseResolutionError("itch.io Linux upload has no valid discovered upload id")
    return upload


def _csrf_token(page_html: str) -> str:
    match = re.search(
        r"<meta\b[^>]*\bname=[\"']csrf_token[\"'][^>]*\bvalue=[\"']([^\"']+)",
        page_html,
        flags=re.IGNORECASE,
    )
    if match is None:
        # Attribute order is not a contractual part of HTML.  Accommodate
        # itch templates that place value before name while still requiring a
        # real token rather than inventing one.
        match = re.search(
            r"<meta\b[^>]*\bvalue=[\"']([^\"']+)[\"'][^>]*\bname=[\"']csrf_token[\"']",
            page_html,
            flags=re.IGNORECASE,
        )
    if match is None or not match.group(1).strip():
        raise ReleaseResolutionError("itch.io page has no CSRF token")
    return match.group(1)


def _response_status(response: Any) -> int:
    status = getattr(response, "status", None)
    if status is None:
        status = getattr(response, "code", 200)
    try:
        return int(status)
    except (TypeError, ValueError):
        return 200


def _validate_https_host(url: str, *, allowed_hosts: tuple[str, ...], purpose: str) -> None:
    parsed = urllib.parse.urlparse(url)
    host = (parsed.hostname or "").lower().rstrip(".")
    if parsed.scheme.lower() != "https":
        raise ReleaseResolutionError(f"{purpose} must use HTTPS: {url}")
    if not host or not any(
        host == item or (item.startswith(".") and host.endswith(item))
        for item in allowed_hosts
    ):
        raise ReleaseResolutionError(f"{purpose} has an untrusted host: {host or '<missing>'}")


def _fetch_bytes(
    url: str,
    *,
    opener: Callable[..., Any],
    method: str = "GET",
    form: Mapping[str, str] | None = None,
    purpose: str,
    timeout: int = 60,
    allowed_hosts: tuple[str, ...] | None = None,
    max_bytes: int = MAX_NEGOTIATION_RESPONSE_BYTES,
) -> bytes:
    data = None
    headers = {
        "Accept": "text/html,application/json,application/octet-stream;q=0.9,*/*;q=0.1",
        "User-Agent": "mindustry-compat-resolver",
    }
    if form is not None:
        data = urllib.parse.urlencode(form).encode("utf-8")
        headers["Content-Type"] = "application/x-www-form-urlencoded"
        headers["X-Requested-With"] = "XMLHttpRequest"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    if allowed_hosts is not None:
        _validate_https_host(url, allowed_hosts=allowed_hosts, purpose=purpose)
    try:
        with opener(request, timeout=timeout) as response:
            status = _response_status(response)
            if status != 200:
                raise ReleaseResolutionError(f"{purpose} failed with HTTP {status}: {url}")
            final_url = getattr(response, "geturl", lambda: url)()
            if allowed_hosts is not None:
                _validate_https_host(final_url, allowed_hosts=allowed_hosts, purpose=purpose)
            total = 0
            chunks: list[bytes] = []
            while True:
                chunk = response.read(min(1024 * 1024, max_bytes - total + 1))
                if not chunk:
                    break
                if not isinstance(chunk, (bytes, bytearray)):
                    raise ReleaseResolutionError(f"{purpose} returned non-binary data")
                total += len(chunk)
                if total > max_bytes:
                    raise ReleaseResolutionError(
                        f"{purpose} response exceeds {max_bytes} bytes"
                    )
                chunks.append(bytes(chunk))
            return b"".join(chunks)
    except ReleaseResolutionError:
        raise
    except (
        http.client.IncompleteRead,
        http.client.HTTPException,
        urllib.error.HTTPError,
        urllib.error.URLError,
        TimeoutError,
        ConnectionError,
        OSError,
    ) as exc:
        raise ReleaseResolutionError(f"{purpose} failed: {exc}") from exc


def _fetch_text(
    url: str,
    *,
    opener: Callable[..., Any],
    purpose: str,
    allowed_hosts: tuple[str, ...] | None = None,
) -> str:
    try:
        return _fetch_bytes(
            url, opener=opener, purpose=purpose, allowed_hosts=allowed_hosts
        ).decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ReleaseResolutionError(f"{purpose} was not UTF-8 HTML") from exc


def _post_json(
    url: str,
    *,
    csrf: str,
    opener: Callable[..., Any],
    purpose: str,
    allowed_hosts: tuple[str, ...] | None = None,
) -> Mapping[str, Any]:
    payload = _fetch_bytes(
        url,
        opener=opener,
        method="POST",
        form={"csrf_token": csrf},
        purpose=purpose,
        allowed_hosts=allowed_hosts,
    )
    try:
        value = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise ReleaseResolutionError(f"{purpose} returned invalid JSON") from exc
    if not isinstance(value, Mapping):
        raise ReleaseResolutionError(f"{purpose} returned a non-object response")
    if value.get("errors"):
        raise ReleaseResolutionError(f"{purpose} returned errors: {value['errors']!r}")
    return value


def resolve_itch_download(
    target: CurrentTarget,
    *,
    opener: Callable[..., Any] = _urlopen,
    landing_html: str | None = None,
    file_url: str | None = None,
) -> ItchDownload:
    """Resolve the exact itch Linux upload into a signed archive URL.

    ``landing_html`` and ``file_url`` are injectable hermetic seams used by
    self-tests.  Production calls discover both from itch.io at runtime.
    """

    landing_url = f"{ITCH_GAME_URL}/download"
    if landing_html is None:
        purchase_html = _fetch_text(
            f"{ITCH_GAME_URL}/purchase",
            opener=opener,
            purpose="itch.io purchase page",
            allowed_hosts=ITCH_LANDING_HOSTS,
        )
        csrf = _csrf_token(purchase_html)
        token = _post_json(
            f"{ITCH_GAME_URL}/download_url",
            csrf=csrf,
            opener=opener,
            purpose="itch.io download token",
            allowed_hosts=ITCH_LANDING_HOSTS,
        )
        landing_url = token.get("url")
        if not isinstance(landing_url, str) or not landing_url.startswith(("https://", "http://")):
            raise ReleaseResolutionError("itch.io download token has no valid landing URL")
        landing_html = _fetch_text(
            landing_url,
            opener=opener,
            purpose="itch.io signed download page",
            allowed_hosts=ITCH_LANDING_HOSTS,
        )

    upload = select_itch_upload(landing_html, target)
    if file_url is None:
        csrf = _csrf_token(landing_html)
        payload = _post_json(
            f"{ITCH_GAME_URL}/file/{upload.upload_id}?source=compat-resolver&as_props=1",
            csrf=csrf,
            opener=opener,
            purpose=f"itch.io Linux upload {upload.upload_id}",
            allowed_hosts=ITCH_LANDING_HOSTS,
        )
        if payload.get("external") is True:
            raise ReleaseResolutionError(
                "itch.io Linux upload resolved to an external download, not the official archive"
            )
        file_url = payload.get("url")
    if not isinstance(file_url, str):
        raise ReleaseResolutionError("itch.io Linux upload returned no valid archive URL")
    _validate_https_host(
        file_url,
        allowed_hosts=ITCH_CDN_SUFFIXES,
        purpose="itch.io Linux archive URL",
    )
    return ItchDownload(upload=upload, landing_url=landing_url, archive_url=file_url)


def _api_asset(raw: Mapping[str, Any]) -> ReleaseAsset:
    try:
        name = str(raw["name"])
        url = str(raw["browser_download_url"])
    except (KeyError, TypeError) as exc:
        raise ReleaseResolutionError("release asset has no name/download URL") from exc
    if not name or not url.startswith(("https://", "http://")):
        raise ReleaseResolutionError(f"release asset has invalid name or URL: {raw!r}")
    size = raw.get("size")
    if size is not None:
        try:
            size = int(size)
        except (TypeError, ValueError) as exc:
            raise ReleaseResolutionError(f"release asset has invalid size: {raw!r}") from exc
        if size < 0:
            raise ReleaseResolutionError(f"release asset has invalid size: {raw!r}")
    return ReleaseAsset(name=name, download_url=url, size=size)


def _validate_asset_metadata(asset: ReleaseAsset, target: CurrentTarget) -> ReleaseAsset:
    """Reject a known-size mismatch before spending time on a large download."""

    if asset.size is not None and asset.size != target.jar_size_bytes:
        raise ReleaseResolutionError(
            f"release asset size mismatch before download: {asset.name} reports "
            f"{asset.size}, expected {target.jar_size_bytes}"
        )
    return asset


def select_release_asset(
    metadata: Mapping[str, Any], target: CurrentTarget, *, check_size: bool = True
) -> ReleaseAsset:
    """Select exactly one official desktop JAR from release API metadata.

    GitHub's v159.7 release names this desktop artifact ``Mindustry.jar`` while
    local distributions may rename it to ``159.7.jar``.  The small alias set
    accepts either official naming convention, and any second matching asset
    is an error rather than an arbitrary choice.
    """

    if metadata.get("tag_name") != target.source_tag:
        raise ReleaseResolutionError(
            f"release tag mismatch: requested {target.source_tag}, got {metadata.get('tag_name')!r}"
        )
    raw_assets = metadata.get("assets")
    if not isinstance(raw_assets, list):
        raise ReleaseResolutionError("release metadata has no assets list")
    assets = [_api_asset(raw) for raw in raw_assets if isinstance(raw, Mapping)]
    if len(assets) != len(raw_assets):
        raise ReleaseResolutionError("release metadata contains malformed asset entries")

    # Official desktop release names used by Mindustry releases.  This is a
    # name mapping only; the resulting bytes are never used by the current
    # resolver and the itch artifact remains the binary authority.
    aliases = {target.jar_filename, "Mindustry.jar", "mindustry.jar", "desktop.jar"}
    desktop = [asset for asset in assets if asset.name in aliases]
    if len(desktop) == 1:
        return _validate_asset_metadata(desktop[0], target) if check_size else desktop[0]
    if len(desktop) > 1:
        names = ", ".join(asset.name for asset in desktop)
        raise ReleaseResolutionError(f"ambiguous desktop release assets: {names}")
    raise ReleaseResolutionError(
        f"official desktop JAR asset not found for {target.source_tag} (expected {target.jar_filename!r})"
    )


def _fetch_json(url: str, *, opener: Callable[..., Any], purpose: str) -> Mapping[str, Any]:
    request = urllib.request.Request(
        url,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "mindustry-compat-resolver"},
    )
    try:
        _validate_https_host(url, allowed_hosts=GITHUB_API_HOSTS, purpose=purpose)
        with opener(request, timeout=60) as response:
            status = getattr(response, "status", 200)
            if status != 200:
                raise ReleaseResolutionError(f"{purpose} failed with HTTP {status}: {url}")
            _validate_https_host(
                getattr(response, "geturl", lambda: url)(),
                allowed_hosts=GITHUB_API_HOSTS,
                purpose=purpose,
            )
            payload = json.load(response)
    except ReleaseResolutionError:
        raise
    except urllib.error.HTTPError as exc:
        raise ReleaseResolutionError(f"{purpose} failed with HTTP {exc.code}: {url}") from exc
    except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as exc:
        raise ReleaseResolutionError(f"{purpose} failed: {exc}") from exc
    if not isinstance(payload, Mapping):
        raise ReleaseResolutionError(f"{purpose} returned a non-object response")
    return payload


def fetch_release_metadata(
    target: CurrentTarget,
    *,
    repository: str = DEFAULT_REPOSITORY,
    opener: Callable[..., Any] = _urlopen,
) -> Mapping[str, Any]:
    """Fetch one exact GitHub release; never fall back to another tag."""

    url = f"{GITHUB_API}/repos/{repository}/releases/tags/{target.source_tag}"
    return _fetch_json(url, opener=opener, purpose=f"release lookup for exact tag {target.source_tag}")


def fetch_tag_commit(
    target: CurrentTarget,
    *,
    repository: str = DEFAULT_REPOSITORY,
    opener: Callable[..., Any] = _urlopen,
) -> str:
    """Resolve an exact GitHub tag ref (including annotated tags) to a commit."""

    ref_url = f"{GITHUB_API}/repos/{repository}/git/ref/tags/{target.source_tag}"
    ref = _fetch_json(ref_url, opener=opener, purpose=f"tag lookup for {target.source_tag}")
    return validate_tag_commit_metadata(target, ref, repository=repository, opener=opener)


def validate_tag_commit_metadata(
    target: CurrentTarget,
    ref: Mapping[str, Any],
    *,
    repository: str = DEFAULT_REPOSITORY,
    opener: Callable[..., Any] = _urlopen,
) -> str:
    """Validate an exact GitHub tag-ref payload (including annotated tags)."""

    expected_ref = f"refs/tags/{target.source_tag}"
    if ref.get("ref") != expected_ref:
        raise ReleaseResolutionError(
            f"tag ref mismatch: requested {expected_ref}, got {ref.get('ref')!r}"
        )
    obj = ref.get("object")
    if not isinstance(obj, Mapping) or not obj.get("sha") or obj.get("type") not in {"commit", "tag"}:
        raise ReleaseResolutionError(f"tag ref has no usable object for {target.source_tag}")
    commit_sha = str(obj["sha"])
    if obj.get("type") == "tag":
        tag_url = f"{GITHUB_API}/repos/{repository}/git/tags/{commit_sha}"
        tag = _fetch_json(tag_url, opener=opener, purpose=f"annotated tag lookup for {target.source_tag}")
        tag_obj = tag.get("object")
        if not isinstance(tag_obj, Mapping) or tag_obj.get("type") != "commit" or not tag_obj.get("sha"):
            raise ReleaseResolutionError(f"annotated tag does not point to a commit: {target.source_tag}")
        commit_sha = str(tag_obj["sha"])
    if commit_sha.lower() != target.source_commit.lower():
        raise ReleaseResolutionError(
            f"source commit mismatch for {target.source_tag}: {commit_sha} != {target.source_commit}"
        )
    return commit_sha


def download_asset(
    asset: ReleaseAsset,
    destination: Path,
    *,
    opener: Callable[..., Any] = _urlopen,
) -> None:
    """Download an asset atomically, rejecting HTTP/error responses later by identity validation."""

    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(
        prefix=f".{destination.name}.", suffix=".part", dir=str(destination.parent)
    )
    os.close(fd)
    temporary_path = Path(temporary)
    request = urllib.request.Request(
        asset.download_url,
        headers={"Accept": "application/octet-stream", "User-Agent": "mindustry-compat-resolver"},
    )
    try:
        with opener(request, timeout=120) as response, temporary_path.open("wb") as output:
            status = getattr(response, "status", 200)
            if status != 200:
                raise ReleaseResolutionError(
                    f"JAR download failed with HTTP {status}: {asset.download_url}"
                )
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                output.write(chunk)
        os.replace(temporary_path, destination)
    except ReleaseResolutionError:
        temporary_path.unlink(missing_ok=True)
        raise
    except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError, OSError) as exc:
        temporary_path.unlink(missing_ok=True)
        raise ReleaseResolutionError(f"JAR download failed: {exc}") from exc


def download_itch_archive(
    archive_url: str,
    destination: Path,
    *,
    opener: Callable[..., Any] = _urlopen,
) -> None:
    """Download an itch archive to ``destination`` using an atomic rename."""

    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(
        prefix=f".{destination.name}.", suffix=".part", dir=str(destination.parent)
    )
    os.close(fd)
    temporary_path = Path(temporary)
    request = urllib.request.Request(
        archive_url,
        headers={
            "Accept": "application/zip,application/octet-stream;q=0.9,*/*;q=0.1",
            "User-Agent": "mindustry-compat-resolver",
        },
    )
    try:
        _validate_https_host(
            archive_url,
            allowed_hosts=ITCH_CDN_SUFFIXES,
            purpose="itch.io archive URL",
        )
        with opener(request, timeout=180) as response, temporary_path.open("wb") as output:
            status = _response_status(response)
            if status != 200:
                raise ReleaseResolutionError(
                    f"itch.io archive download failed with HTTP {status}: {archive_url}"
                )
            final_url = getattr(response, "geturl", lambda: archive_url)()
            _validate_https_host(
                final_url,
                allowed_hosts=ITCH_CDN_SUFFIXES,
                purpose="itch.io archive redirect",
            )
            headers = getattr(response, "headers", None)
            declared_length = None
            if headers is not None:
                raw_length = headers.get("Content-Length")
                if raw_length is not None:
                    try:
                        declared_length = int(raw_length)
                    except (TypeError, ValueError) as exc:
                        raise ReleaseResolutionError(
                            "itch.io archive has invalid Content-Length"
                        ) from exc
                    if declared_length < 1 or declared_length > MAX_ITCH_ARCHIVE_BYTES:
                        raise ReleaseResolutionError(
                            f"itch.io archive Content-Length exceeds limit: {declared_length}"
                        )
            content_type = ""
            if headers is not None:
                try:
                    content_type = str(headers.get_content_type()).lower()
                except (AttributeError, TypeError):
                    content_type = str(headers.get("Content-Type", "")).split(";", 1)[0].lower()
            if content_type in {"text/html", "text/plain", "application/json"}:
                raise ReleaseResolutionError(
                    f"itch.io archive endpoint returned {content_type}, not a ZIP"
                )
            total = 0
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                if not isinstance(chunk, (bytes, bytearray)):
                    raise ReleaseResolutionError("itch.io archive returned non-binary data")
                total += len(chunk)
                if total > MAX_ITCH_ARCHIVE_BYTES:
                    raise ReleaseResolutionError("itch.io archive exceeds maximum size")
                output.write(chunk)
            if declared_length is not None and total != declared_length:
                raise ReleaseResolutionError(
                    f"itch.io archive truncated: received {total}, expected {declared_length}"
                )
        os.replace(temporary_path, destination)
    except ReleaseResolutionError:
        temporary_path.unlink(missing_ok=True)
        raise
    except (
        http.client.IncompleteRead,
        http.client.HTTPException,
        urllib.error.HTTPError,
        urllib.error.URLError,
        TimeoutError,
        ConnectionError,
        OSError,
    ) as exc:
        temporary_path.unlink(missing_ok=True)
        raise ReleaseResolutionError(f"itch.io archive download failed: {exc}") from exc


def extract_itch_desktop_jar(
    archive_path: Path,
    destination: Path,
    target: CurrentTarget,
) -> JarIdentity:
    """Extract exactly one ``jre/desktop.jar`` and certify it atomically."""

    archive_path = Path(archive_path)
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(
        prefix=f".{destination.name}.", suffix=".part", dir=str(destination.parent)
    )
    os.close(fd)
    temporary_path = Path(temporary)
    try:
        try:
            with zipfile.ZipFile(archive_path) as archive:
                matches = [
                    info for info in archive.infolist() if info.filename == "jre/desktop.jar"
                ]
                if len(matches) != 1 or matches[0].is_dir():
                    raise ReleaseResolutionError(
                        "itch.io Linux archive must contain exactly one file named jre/desktop.jar"
                    )
                if matches[0].file_size != target.jar_size_bytes:
                    raise ReleaseResolutionError(
                        f"jre/desktop.jar member size mismatch: {matches[0].file_size} "
                        f"!= {target.jar_size_bytes}"
                    )
                with archive.open(matches[0], "r") as source, temporary_path.open("wb") as output:
                    while True:
                        chunk = source.read(1024 * 1024)
                        if not chunk:
                            break
                        output.write(chunk)
        except ReleaseResolutionError:
            raise
        except (
            OSError,
            RuntimeError,
            zipfile.BadZipFile,
            zipfile.LargeZipFile,
            NotImplementedError,
            zlib.error,
        ) as exc:
            raise ReleaseResolutionError(f"invalid itch.io Linux ZIP: {exc}") from exc

        identity = verify_current_jar(temporary_path, target)
        os.replace(temporary_path, destination)
        return identity
    except ReleaseResolutionError:
        temporary_path.unlink(missing_ok=True)
        raise
    except OSError as exc:
        temporary_path.unlink(missing_ok=True)
        raise ReleaseResolutionError(f"cannot install certified JAR {destination}: {exc}") from exc


def verify_current_jar(path: Path, target: CurrentTarget) -> JarIdentity:
    """Validate one downloaded/cache JAR against current.toml."""

    try:
        return validate_jar_identity(
            Path(path),
            expected_build=target.build,
            expected_size_bytes=target.jar_size_bytes,
            expected_sha256=target.jar_sha256,
        )
    except JarIdentityError as exc:
        raise ReleaseResolutionError(str(exc)) from exc


def resolve_current_jar(
    *,
    current_path: Path,
    output_dir: Path,
    repository: str = DEFAULT_REPOSITORY,
    opener: Callable[..., Any] = _urlopen,
    itch_landing_html: str | None = None,
    itch_file_url: str | None = None,
    tag_metadata: Mapping[str, Any] | None = None,
    release_metadata: Mapping[str, Any] | None = None,
) -> Path:
    """Return a verified current-target JAR, using the itch Linux distribution.

    GitHub remains the immutable source authority: the tag is resolved on
    every invocation and must point to ``current.toml``'s commit.  The binary
    is intentionally resolved from itch.io because the GitHub release asset
    named ``Mindustry.jar`` is not byte-identical to the target profile.  The
    separate ``tag_metadata`` seam can inject a GitHub tag response in
    hermetic tests, but cannot bypass commit validation.  There is deliberately
    no release-asset override: the current-target binary source is always the
    itch.io Linux distribution.
    """

    target = target_from_file(Path(current_path))
    output_dir = Path(output_dir)
    destination = output_dir / target.jar_filename
    # A cache hit still gets an exact tag-ref check.  The release asset/token
    # lookup is skipped only after both source commit and artifact identity are
    # independently verified.
    if tag_metadata is None:
        fetch_tag_commit(target, repository=repository, opener=opener)
    else:
        validate_tag_commit_metadata(target, tag_metadata, repository=repository, opener=opener)
    # The GitHub release must exist and advertise exactly one official desktop
    # asset, but its bytes are intentionally not used (the itch Linux bundle
    # contains the target desktop JAR).  Do not apply the target JAR size check
    # here: the known GitHub asset is a different packaging artifact.
    release_metadata = release_metadata or fetch_release_metadata(
        target, repository=repository, opener=opener
    )
    select_release_asset(release_metadata, target, check_size=False)
    if destination.exists():
        try:
            verify_current_jar(destination, target)
            return destination
        except ReleaseResolutionError:
            # Never use a bad cache entry.  Replace it only after resolving the
            # exact release below; no alternate release/build is considered.
            pass

    itch = resolve_itch_download(
        target,
        opener=opener,
        landing_html=itch_landing_html,
        file_url=itch_file_url,
    )
    archive_path = output_dir / f".{target.jar_filename}.itch.zip"
    try:
        download_itch_archive(itch.archive_url, archive_path, opener=opener)
        extract_itch_desktop_jar(archive_path, destination, target)
    finally:
        archive_path.unlink(missing_ok=True)
    return destination


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--current",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "compat" / "current.toml",
        help="compatibility target TOML (default: compat/current.toml)",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(".cache/mindustry"),
        help="download/cache directory",
    )
    parser.add_argument("--repository", default=DEFAULT_REPOSITORY, help="GitHub owner/repository")
    parser.add_argument(
        "--itch-landing-file",
        type=Path,
        help="local itch signed-download HTML fixture (hermetic tests; skips token lookup)",
    )
    parser.add_argument(
        "--itch-file-url",
        help="inject the resolved itch archive URL (hermetic tests; requires --itch-landing-file)",
    )
    parser.add_argument(
        "--tag-metadata-file",
        type=Path,
        help="local exact GitHub tag-ref JSON fixture (hermetic tests; still commit-validated)",
    )
    parser.add_argument("--quiet", action="store_true", help="print only the resolved path")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        tag_metadata = None
        if args.tag_metadata_file is not None:
            with args.tag_metadata_file.open(encoding="utf-8") as stream:
                loaded_tag = json.load(stream)
            if not isinstance(loaded_tag, Mapping):
                raise ReleaseResolutionError("tag metadata fixture must contain a JSON object")
            tag_metadata = loaded_tag
        if args.itch_file_url is not None and args.itch_landing_file is None:
            raise ReleaseResolutionError("--itch-file-url requires --itch-landing-file")
        landing_html = (
            args.itch_landing_file.read_text(encoding="utf-8")
            if args.itch_landing_file is not None
            else None
        )
        target = target_from_file(args.current)
        path = resolve_current_jar(
            current_path=args.current,
            output_dir=args.output_dir,
            repository=args.repository,
            itch_landing_html=landing_html,
            itch_file_url=args.itch_file_url,
            tag_metadata=tag_metadata,
        )
        if not args.quiet:
            print(
                f"current target {target.build} verified: {path} "
                f"size={target.jar_size_bytes} sha256={target.jar_sha256}",
                file=sys.stderr,
            )
        print(path)
        return 0
    except (OSError, json.JSONDecodeError, ReleaseResolutionError) as exc:
        print(f"current JAR resolution failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
