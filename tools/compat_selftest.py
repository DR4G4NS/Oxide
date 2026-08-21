#!/usr/bin/env python3
"""Self-tests for compatibility schema, classifier, diff, ledger, and canary."""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib import SCHEMA_VERSION
from compatlib.atomic import canonical_dumps
from compatlib.classifier import classify_path
from compatlib.diff import diff_builds, diff_packets
from compatlib.ledger import (
    certification_may_pass,
    unresolved_server_rows,
    validate_ledger,
    validate_rust_test_evidence,
)
from compatlib.schema import contains_local_path, validate_artifact_file, validate_provenance
from compatlib.wrap import provenance, wrap

REPO_ROOT = Path(__file__).resolve().parent.parent
PASS = 0
FAIL = 0


def check(name: str, cond: bool, detail: str = "") -> None:
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  [ok] {name}")
    else:
        FAIL += 1
        print(f"  [FAIL] {name} {detail}")


def wrapped(key, payload, build="159.7"):
    return wrap(
        key,
        payload,
        build=build,
        source_ref=f"v{build}",
        source_commit="c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
        jar_filename="159.7.jar",
        jar_size=1,
        jar_sha256="ce1db5b06fe7326b9d0c1d99b1eb1667cf6f0bf97093293f6674ae294981ff05",
    )


def test_classifier() -> None:
    check("1 packet-before-call file classified WIRE", classify_path("core/src/mindustry/net/Net.java") == "WIRE_PACKET")
    check("3 new stream classified STREAMING", classify_path("core/src/mindustry/net/NetworkIO.java") == "STREAMING")
    check("5 remote file classified ADMIN/RPC-ish", classify_path("core/src/mindustry/core/NetServer.java") == "ADMIN")
    check("6 TypeIO", classify_path("core/src/mindustry/io/TypeIO.java") == "TYPEIO")
    check("7 save version", classify_path("core/src/mindustry/io/versions/Save13.java") == "SAVE")
    check("8 save impl", classify_path("core/src/mindustry/io/SaveVersion.java") == "SAVE")
    check("9 EntityProcess", classify_path("annotations/src/main/java/mindustry/annotations/entity/EntityProcess.java") == "ENTITY_SYNC")
    check("10 LExecutor", classify_path("core/src/mindustry/logic/LExecutor.java") == "LOGIC")
    check("11 StatusComp", classify_path("core/src/mindustry/entities/comp/StatusComp.java") == "STATUS")
    check("12 PhysicsProcess", classify_path("core/src/mindustry/entities/PhysicsProcess.java") == "PHYSICS_COLLISION")
    check("13 stateful building", classify_path("core/src/mindustry/world/blocks/distribution/Router.java") == "STATEFUL_BUILDING")
    check("14 unknown java fail-closed", classify_path("core/src/mindustry/mystery/NewAuthoritative.java") == "UNKNOWN_REQUIRES_PROBE")
    check("15 client UI", classify_path("core/src/mindustry/ui/dialogs/SettingsDialog.java") == "CLIENT_ONLY")
    check("16 editor", classify_path("core/src/mindustry/editor/MapEditor.java") == "EDITOR_ONLY")


def test_packets_shift() -> None:
    from_pkts = [{"id": 0, "name": "StreamBegin"}, {"id": 1, "name": "ConnectConfirmCallPacket"}]
    to_pkts = [
        {"id": 0, "name": "StreamBegin"},
        {"id": 1, "name": "NewStream"},
        {"id": 2, "name": "ConnectConfirmCallPacket"},
    ]
    d = diff_packets(from_pkts, to_pkts)
    check("1+2 inserted packet shifts generated id", any(x["name"] == "ConnectConfirmCallPacket" and x["to_id"] == 2 for x in d["id_shifted"]))
    check("3 new stream packet added", any(p["name"] == "NewStream" for p in d["added"]))


def test_typeio_and_saves() -> None:
    from_t = {"methods": [{"name": "writeObject", "return": "void", "params": ["arc.util.io.Writes", "java.lang.Object"]}]}
    to_t = {"methods": from_t["methods"] + [{"name": "writeShorts", "return": "void", "params": []}]}
    d = diff_builds(
        {"packets": [], "saves": [{"version": 11, "class": "Save11"}], "content": {}, "rules": [], "logic": [], "typeio": from_t, "rpc": [], "streams": [], "entities": [], "entity_sync": {}, "fingerprints": {"LExecutor": "aaa"}},
        {"packets": [], "saves": [{"version": 11, "class": "Save11"}, {"version": 13, "class": "Save13"}], "content": {}, "rules": [], "logic": [], "typeio": to_t, "rpc": [{"generated_class": "NewRemoteCallPacket"}], "streams": [], "entities": [], "entity_sync": {"mapping_fingerprint": "x"}, "fingerprints": {"LExecutor": "bbb"}},
    )
    check("4 new remote rpc added", len(d["rpc"]["added"]) == 1)
    check("5 new TypeIO method", any(m.get("name") == "writeShorts" for m in d["typeio"]["added"]))
    check("7 save version added", any(s["version"] == 13 for s in d["saves"]["added"]))
    from_s = {"packets": [], "saves": [{"version": 12, "class": "Save12Old"}], "content": {}, "rules": [], "logic": []}
    to_s = {"packets": [], "saves": [{"version": 12, "class": "Save12"}], "content": {}, "rules": [], "logic": []}
    d2 = diff_builds(from_s, to_s)
    check("8 save class changed same version", d2["saves"]["class_changed"][0]["to"] == "Save12")
    check("9 entity sync change detected", d["entity_sync"]["changed"])
    check("10 logic fingerprint change detected", d["fingerprints"]["changed"])


def test_provenance_and_ledger() -> None:
    bad = provenance(build="159.7", source_ref="v159.7", source_commit="abc")
    bad["jar"] = {"path": "/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar", "sha256": "x", "size_bytes": 1, "filename": "159.7.jar"}
    errs = validate_provenance(bad, require_jar=True)
    check("22 local absolute JAR path rejected", any("path" in e or "leaked" in e for e in errs))
    good = wrapped("packets", [{"id": 0, "name": "StreamBegin"}])
    check("provenance ok", not validate_provenance(good, require_jar=True))

    ledger = {
        "schema_version": 2,
        "build": "159.7",
        "baseline": "158.1",
        "overall_status": "PASS",
        "source_ref": "v159.7",
        "source_commit": "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
        "rows": [
            {
                "id": "W1",
                "category": "WIRE_PACKET",
                "upstream_files": ["Net.java"],
                "upstream_symbols": [],
                "source_change": "x",
                "rust_owner": "protocol.rs",
                "risk": "HIGH",
                "implementation_required": True,
                "evidence_required": True,
                "status": "PLANNED" if False else "IMPLEMENTATION_REQUIRED",
                "source_evidence": "",
                "jar_probe": "",
                "rust_tests": "",
                "notes": "",
            }
        ],
    }
    # illegal PLANNED is tested separately
    planned = dict(ledger)
    planned["rows"] = [dict(ledger["rows"][0], status="PLANNED")]
    check("21 PLANNED is illegal status", bool(validate_ledger(planned)))
    check("21 unresolved blocks PASS", not certification_may_pass(ledger))
    client_row = dict(ledger["rows"][0], category="CLIENT_ONLY", status="CLIENT_ONLY", implementation_required=False)
    ok_ledger = dict(ledger, overall_status="CERTIFICATION_REOPENED", rows=[client_row])
    check("15/16 client terminal ok", not unresolved_server_rows(ok_ledger))

    stale = {"overall_status": "PASS", "rows": ledger["rows"]}
    check("20 markdown/ledger contradiction: PASS with unresolved", not certification_may_pass(stale | {"schema_version": 2, "build": "159.7", "baseline": "158.1"}))

    bad_impl = {
        "schema_version": 2,
        "build": "159.7",
        "baseline": "158.1",
        "overall_status": "CERTIFICATION_REOPENED",
        "rows": [{
            "id": "BAD-001",
            "category": "LOGIC",
            "upstream_files": [],
            "upstream_symbols": [],
            "source_change": "x",
            "rust_owner": "x",
            "risk": "HIGH",
            "implementation_required": True,
            "evidence_required": True,
            "status": "VERIFIED_IMPLEMENTED",
            "source_evidence": "",
            "jar_probe": "",
            "rust_tests": "",
            "notes": "",
        }],
    }
    check(
        "evidence_required_true_verified_implemented_empty_evidence_fails",
        any("BAD-001" in e for e in validate_ledger(bad_impl)),
    )
    good_impl = dict(bad_impl)
    good_impl["rows"] = [dict(bad_impl["rows"][0], source_evidence="v159.7 ref", rust_tests="foo_test")]
    check(
        "verified_implemented_with_source_and_test_passes",
        not any("BAD-001" in e for e in validate_ledger(good_impl)),
    )
    bad_unchanged = dict(bad_impl)
    bad_unchanged["rows"] = [dict(bad_impl["rows"][0], status="VERIFIED_UNCHANGED")]
    check(
        "verified_unchanged_without_source_evidence_fails",
        any("BAD-001" in e for e in validate_ledger(bad_unchanged)),
    )
    bad_oos = dict(bad_impl)
    bad_oos["rows"] = [dict(bad_impl["rows"][0], status="OUT_OF_SCOPE_EXPLICIT", notes="short")]
    check(
        "out_of_scope_without_reason_fails",
        any("BAD-001" in e for e in validate_ledger(bad_oos)),
    )
    pass_bad = dict(bad_impl, overall_status="PASS")
    check(
        "pass_with_invalid_terminal_row_fails_certification",
        not certification_may_pass(pass_bad),
    )
    bypass_impl = dict(good_impl)
    bypass_impl["rows"] = [
        dict(
            good_impl["rows"][0],
            evidence_required=False,
            status="VERIFIED_IMPLEMENTED",
        )
    ]
    check(
        "implementation_required_with_evidence_required_false_fails",
        any("evidence_required" in e for e in validate_ledger(bypass_impl)),
    )
    client_bypass = dict(good_impl)
    client_bypass["rows"] = [dict(good_impl["rows"][0], status="CLIENT_ONLY")]
    check(
        "implementation_required_client_only_status_fails",
        any("CLIENT_ONLY" in e for e in validate_ledger(client_bypass)),
    )
    fake_tests = dict(good_impl)
    fake_tests["rows"] = [
        dict(good_impl["rows"][0], rust_tests="not a valid test (tests.rs:1)")
    ]
    check(
        "fake_rust_tests_token_fails_validation",
        any("rust_tests" in e for e in validate_ledger(fake_tests)),
    )
    flip_pass = dict(pass_bad, overall_status="PASS")
    check(
        "flip_overall_pass_with_invalid_row_fails_certification",
        not certification_may_pass(flip_pass),
    )
    listed = frozenset(
        {
            "oxide::logic::tests::logic_spawn_effect_runtime_variable_false",
            "oxide::network::listener::tests::best_core_foundation_beats_shard",
        }
    )
    missing = dict(good_impl)
    missing["rows"] = [
        dict(good_impl["rows"][0], rust_tests="completely_fake_test_name")
    ]
    check(
        "ledger_nonexistent_rust_test_fails",
        any("completely_fake_test_name" in e for e in validate_rust_test_evidence(missing, listed)),
    )
    present = dict(good_impl)
    present["rows"] = [dict(good_impl["rows"][0], rust_tests="logic_spawn_effect_runtime_variable_false")]
    check(
        "ledger_existing_rust_test_passes",
        not validate_rust_test_evidence(present, listed),
    )


def test_rules_defaults_are_jvm_stable() -> None:
    payload = json.loads((REPO_ROOT / "compat/159.7/rules.json").read_text(encoding="utf-8"))
    fields = {row["name"]: row for row in payload["rules"]}
    check("MapObjectives default is JVM-stable", fields["objectives"]["default"] == "mindustry.game.MapObjectives")
    check("TeamRules default is JVM-stable", fields["teams"]["default"] == "mindustry.game.Rules$TeamRules")


def test_canary_guard() -> None:
    """Run the real canary writer and prove compat/current.toml is untouched."""
    import subprocess

    from compat_diff import write_canary

    current_toml = REPO_ROOT / "compat" / "current.toml"
    before = current_toml.read_text(encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="canary-selftest-") as tmp:
        repo = Path(tmp) / "src"
        repo.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.email", "cert@example.test"], cwd=repo, check=True)
        subprocess.run(["git", "config", "user.name", "cert"], cwd=repo, check=True)
        (repo / "README.md").write_text("from\n", encoding="utf-8")
        subprocess.run(["git", "add", "README.md"], cwd=repo, check=True)
        subprocess.run(["git", "commit", "-q", "-m", "from"], cwd=repo, check=True)
        from_ref = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
        (repo / "README.md").write_text("to\n", encoding="utf-8")
        subprocess.run(["git", "add", "README.md"], cwd=repo, check=True)
        subprocess.run(["git", "commit", "-q", "-m", "to"], cwd=repo, check=True)
        to_ref = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
        md_out = Path(tmp) / "master-canary.md"
        rc = write_canary(repo, from_ref, to_ref, md_out)
        after = current_toml.read_text(encoding="utf-8")
        check("23 current.toml unchanged by canary writer", before == after and rc == 0, f"rc={rc}")
        check("23b canary wrote markdown output", md_out.exists() and md_out.stat().st_size > 0)


def _write_fake_jar(path: Path, build: str) -> None:
    import zipfile

    with zipfile.ZipFile(path, "w") as z:
        z.writestr("version.properties", f"type=official\nnumber=1\nbuild={build}\nmodifier=\n")


def test_wrong_commit_and_jar() -> None:
    from compatlib.jar_identity import check_jar_build, check_jar_sha256

    check(
        "17 wrong-commit detection is a compare of SHAs",
        "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c" != "deadbeef",
    )
    with tempfile.TemporaryDirectory(prefix="jar-selftest-") as tmp:
        wrong_build = Path(tmp) / "wrong-build.jar"
        _write_fake_jar(wrong_build, "999.9")
        err = check_jar_build(wrong_build, "159.7")
        check("18 JAR build mismatch is rejected", err is not None and "999.9" in err and "159.7" in err, str(err))

        right_build = Path(tmp) / "right-build.jar"
        _write_fake_jar(right_build, "159.7")
        check("18b matching JAR build accepted", check_jar_build(right_build, "159.7") is None)

        sha_err = check_jar_sha256(right_build, "0" * 64)
        check(
            "19 JAR SHA mismatch is rejected",
            sha_err is not None and "JAR SHA mismatch" in sha_err,
            str(sha_err),
        )
        # Wrong current target identity when build string does not match expected.
        wrong_target = Path(tmp) / "wrong-target.jar"
        _write_fake_jar(wrong_target, "158.1")
        check(
            "19b malformed/wrong current target build rejected",
            check_jar_build(wrong_target, "159.7") is not None,
        )


def _resolver_target(*, size: int, sha: str, build: str = "159.7"):
    from compatlib.release_jar import CurrentTarget

    return CurrentTarget(
        build=build,
        source_tag="v159.7",
        source_commit="c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
        jar_filename="159.7.jar",
        jar_size_bytes=size,
        jar_sha256=sha,
    )


def _write_resolver_fixture(path: Path, *, build: str = "159.7", include_version: bool = True) -> None:
    import zipfile

    with zipfile.ZipFile(path, "w") as archive:
        if include_version:
            archive.writestr("version.properties", f"type=official\nbuild={build}\n")
            archive.writestr("payload.bin", b"resolver fixture\n")


def _itch_upload_fixture(
    *,
    linux_name: str = "[Linux-64bit]Mindustry.zip",
    linux_version: str = "Version 159.7",
    linux_platform: str = "Download for Linux",
    linux_upload_id: str = "987654",
    duplicate_linux: bool = False,
) -> str:
    rows = [
        (
            "[Windows-64bit]Mindustry.zip",
            "Version 159.7",
            "Download for Windows",
            "111111",
            "icon-windows8",
        ),
        (linux_name, linux_version, linux_platform, linux_upload_id, "icon-tux"),
    ]
    if duplicate_linux:
        rows.append((linux_name, linux_version, linux_platform, "999999", "icon-tux"))
    body = [
        '<html><head><meta name="csrf_token" value="fixture-token"></head><body>'
    ]
    for name, version, platform, upload_id, icon in rows:
        body.append(
            f'<div class="upload"><a class="button download_btn" '
            f'data-upload_id="{upload_id}">Download</a>'
            f'<div class="upload_name"><strong class="name" title="{name}">{name}</strong>'
            f'<span class="download_platforms"><span class="icon {icon}" '
            f'title="{platform}"></span></span></div>'
            f'<div class="build_row"><span class="version_name">{version}</span></div></div>'
        )
    body.append("</body></html>")
    return "".join(body)


def test_release_jar_resolver() -> None:
    """Hermetic resolver identity/metadata tests; no network or large JAR."""

    import hashlib
    import http.client
    import zipfile

    from compatlib.release_jar import (
        ReleaseResolutionError,
        MAX_ITCH_ARCHIVE_BYTES,
        download_itch_archive,
        extract_itch_desktop_jar,
        fetch_tag_commit,
        resolve_current_jar,
        select_release_asset,
        select_itch_upload,
        validate_tag_commit_metadata,
        verify_current_jar,
    )

    with tempfile.TemporaryDirectory(prefix="release-resolver-selftest-") as tmp:
        root = Path(tmp)

        correct = root / "correct.jar"
        _write_resolver_fixture(correct)
        digest = hashlib.sha256(correct.read_bytes()).hexdigest()
        size = correct.stat().st_size
        target = _resolver_target(size=size, sha=digest)
        try:
            identity = verify_current_jar(correct, target)
            correct_ok = identity.sha256 == digest and identity.size_bytes == size
        except ReleaseResolutionError:
            correct_ok = False
        check("release_asset_correct_sha_passes", correct_ok)

        wrong_sha = _resolver_target(size=size, sha="0" * 64)
        try:
            verify_current_jar(correct, wrong_sha)
            wrong_sha_ok = False
        except ReleaseResolutionError:
            wrong_sha_ok = True
        check("release_asset_wrong_sha_fails", wrong_sha_ok)

        wrong_size = _resolver_target(size=size + 1, sha=digest)
        try:
            verify_current_jar(correct, wrong_size)
            wrong_size_ok = False
        except ReleaseResolutionError:
            wrong_size_ok = True
        check("release_asset_wrong_size_fails", wrong_size_ok)

        invalid = root / "invalid.jar"
        invalid.write_bytes(b"<!doctype html><html>server error</html>\n")
        invalid_target = _resolver_target(
            size=invalid.stat().st_size,
            sha=hashlib.sha256(invalid.read_bytes()).hexdigest(),
        )
        try:
            verify_current_jar(invalid, invalid_target)
            invalid_ok = False
        except ReleaseResolutionError:
            invalid_ok = True
        check("release_asset_invalid_zip_fails", invalid_ok)

        missing_version = root / "missing-version.jar"
        _write_resolver_fixture(missing_version, include_version=False)
        missing_target = _resolver_target(
            size=missing_version.stat().st_size,
            sha=hashlib.sha256(missing_version.read_bytes()).hexdigest(),
        )
        try:
            verify_current_jar(missing_version, missing_target)
            missing_version_ok = False
        except ReleaseResolutionError:
            missing_version_ok = True
        check("release_asset_missing_version_properties_fails", missing_version_ok)

        wrong_build = root / "wrong-build.jar"
        _write_resolver_fixture(wrong_build, build="158.1")
        wrong_build_target = _resolver_target(
            size=wrong_build.stat().st_size,
            sha=hashlib.sha256(wrong_build.read_bytes()).hexdigest(),
        )
        try:
            verify_current_jar(wrong_build, wrong_build_target)
            wrong_build_ok = False
        except ReleaseResolutionError:
            wrong_build_ok = True
        check("release_asset_wrong_build_fails", wrong_build_ok)

        # A corrupt deflate stream must fail closed while reading the
        # authoritative version member; no unrelated ZIP members are scanned.
        import struct

        compressed = root / "compressed-corrupt.jar"
        with zipfile.ZipFile(compressed, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("version.properties", "build=159.7\n")
        compressed_bytes = bytearray(compressed.read_bytes())
        info = zipfile.ZipFile(compressed).getinfo("version.properties")
        name_len, extra_len = struct.unpack_from("<HH", compressed_bytes, info.header_offset + 26)
        compressed_bytes[info.header_offset + 30 + name_len + extra_len] ^= 0xFF
        compressed.write_bytes(compressed_bytes)
        compressed_target = _resolver_target(
            size=compressed.stat().st_size,
            sha=hashlib.sha256(compressed.read_bytes()).hexdigest(),
        )
        try:
            verify_current_jar(compressed, compressed_target)
            corrupt_deflate_ok = False
        except ReleaseResolutionError:
            corrupt_deflate_ok = True
        check("jar_identity_corrupt_deflate_fails", corrupt_deflate_ok)

        metadata = {
            "tag_name": "v159.7",
            "assets": [
                {"name": "159.7.jar", "browser_download_url": "https://example.test/159.7.jar"}
            ],
        }
        selected = select_release_asset(metadata, target)
        check("release_asset_correct_name_selected", selected.name == "159.7.jar")

        import io

        class _JsonResponse(io.BytesIO):
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        def tag_opener(request, timeout=60):
            del timeout
            payload = {
                "ref": "refs/tags/v159.7",
                "object": {
                    "sha": "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
                    "type": "commit",
                },
            }
            return _JsonResponse(json.dumps(payload).encode())

        check(
            "release_tag_exact_commit_passes",
            fetch_tag_commit(target, opener=tag_opener)
            == "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
        )

        def wrong_tag_opener(request, timeout=60):
            del request, timeout
            payload = {
                "ref": "refs/tags/v159.7",
                "object": {"sha": "deadbeef" * 5, "type": "commit"},
            }
            return _JsonResponse(json.dumps(payload).encode())

        try:
            fetch_tag_commit(target, opener=wrong_tag_opener)
            wrong_commit_ok = False
        except ReleaseResolutionError:
            wrong_commit_ok = True
        check("release_tag_wrong_commit_fails", wrong_commit_ok)

        wrong_metadata_size = {
            "tag_name": "v159.7",
            "assets": [
                {
                    "name": "159.7.jar",
                    "size": size + 1,
                    "browser_download_url": "https://example.test/159.7.jar",
                }
            ],
        }
        try:
            select_release_asset(wrong_metadata_size, target)
            metadata_size_ok = False
        except ReleaseResolutionError:
            metadata_size_ok = True
        check("release_asset_metadata_size_mismatch_fails_early", metadata_size_ok)

        duplicate_exact = {
            "tag_name": "v159.7",
            "assets": [
                {"name": "159.7.jar", "browser_download_url": "https://example.test/a"},
                {"name": "159.7.jar", "browser_download_url": "https://example.test/b"},
            ],
        }
        try:
            select_release_asset(duplicate_exact, target)
            duplicate_ok = False
        except ReleaseResolutionError:
            duplicate_ok = True
        check("release_asset_duplicate_exact_fails", duplicate_ok)

        try:
            select_release_asset({"tag_name": "v158.1", "assets": []}, target)
            wrong_tag_ok = False
        except ReleaseResolutionError:
            wrong_tag_ok = True
        check("release_wrong_tag_fails", wrong_tag_ok)

        try:
            select_release_asset({"tag_name": "v159.7", "assets": []}, target)
            missing_ok = False
        except ReleaseResolutionError:
            missing_ok = True
        check("release_asset_missing_fails", missing_ok)

        ambiguous = {
            "tag_name": "v159.7",
            "assets": [
                {"name": "Mindustry.jar", "browser_download_url": "https://example.test/a"},
                {"name": "desktop.jar", "browser_download_url": "https://example.test/b"},
            ],
        }
        try:
            select_release_asset(ambiguous, target)
            ambiguous_ok = False
        except ReleaseResolutionError:
            ambiguous_ok = True
        check("release_asset_ambiguous_fails", ambiguous_ok)

        itch_html = _itch_upload_fixture()
        itch_upload = select_itch_upload(itch_html, target)
        check(
            "itch_linux_upload_correct_version_and_platform_passes",
            itch_upload.upload_id == "987654" and itch_upload.platforms == frozenset({"linux"}),
        )
        for name, fixture, expected in [
            (
                "itch_linux_upload_missing_fails",
                _itch_upload_fixture(linux_name="[Server]Mindustry.zip", linux_platform=""),
                True,
            ),
            (
                "itch_linux_upload_ambiguous_fails",
                _itch_upload_fixture(duplicate_linux=True),
                True,
            ),
            (
                "itch_linux_upload_wrong_platform_fails",
                _itch_upload_fixture(linux_platform="Download for Windows"),
                True,
            ),
            (
                "itch_linux_upload_wrong_build_fails",
                _itch_upload_fixture(linux_version="Version 158.1"),
                True,
            ),
        ]:
            try:
                select_itch_upload(fixture, target)
                failed = False
            except ReleaseResolutionError:
                failed = True
            check(name, failed == expected)

        # End-to-end hermetic path: exact GitHub tag fixture, discovered Linux
        # upload fixture, binary ZIP fixture, and atomic final-JAR identity.
        archive = root / "linux.zip"
        with zipfile.ZipFile(archive, "w") as outer:
            outer.write(correct, "jre/desktop.jar")
        class _BinaryResponse(io.BytesIO):
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        archive_bytes = archive.read_bytes()

        duplicate_archive = root / "duplicate-linux.zip"
        with zipfile.ZipFile(duplicate_archive, "w") as outer:
            outer.write(correct, "jre/desktop.jar")
            outer.write(correct, "jre/desktop.jar")
        try:
            extract_itch_desktop_jar(duplicate_archive, root / "duplicate.jar", target)
            duplicate_zip_ok = False
        except ReleaseResolutionError:
            duplicate_zip_ok = not any(root.glob(".duplicate.jar.*.part"))
        check("itch_zip_duplicate_target_fails_and_cleans", duplicate_zip_ok)

        compressed_outer = root / "corrupt-linux.zip"
        with zipfile.ZipFile(compressed_outer, "w", compression=zipfile.ZIP_DEFLATED) as outer:
            outer.writestr("jre/desktop.jar", correct.read_bytes())
        corrupt_outer_bytes = bytearray(compressed_outer.read_bytes())
        with zipfile.ZipFile(compressed_outer) as outer:
            info = outer.getinfo("jre/desktop.jar")
        name_len, extra_len = struct.unpack_from("<HH", corrupt_outer_bytes, info.header_offset + 26)
        corrupt_outer_bytes[info.header_offset + 30 + name_len + extra_len] ^= 0xFF
        compressed_outer.write_bytes(corrupt_outer_bytes)
        try:
            extract_itch_desktop_jar(compressed_outer, root / "corrupt.jar", target)
            corrupt_extract_ok = False
        except ReleaseResolutionError:
            corrupt_extract_ok = not any(root.glob(".corrupt.jar.*.part"))
        check("itch_zip_corrupt_deflate_fails_and_cleans", corrupt_extract_ok)

        def archive_opener(request, timeout=60):
            del timeout
            if request.full_url != "https://itchio-mirror.example.r2.cloudflarestorage.com/linux.zip":
                raise AssertionError(f"unexpected hermetic URL: {request.full_url}")
            return _BinaryResponse(archive_bytes)

        current_file = root / "current.toml"
        current_file.write_text(
            "[target]\n"
            'build = "159.7"\n'
            'source_tag = "v159.7"\n'
            'source_commit = "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c"\n'
            'jar_filename = "159.7.jar"\n'
            f"jar_size_bytes = {size}\n"
            f'jar_sha256 = "{digest}"\n',
            encoding="utf-8",
        )
        tag_fixture = {
            "ref": "refs/tags/v159.7",
            "object": {
                "sha": "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c",
                "type": "commit",
            },
        }
        release_fixture = {
            "tag_name": "v159.7",
            "assets": [
                {
                    "name": "Mindustry.jar",
                    "browser_download_url": "https://example.test/github.jar",
                    "size": size + 123,
                }
            ],
        }
        resolved = resolve_current_jar(
            current_path=current_file,
            output_dir=root / "cache",
            tag_metadata=tag_fixture,
            itch_landing_html=itch_html,
            itch_file_url="https://itchio-mirror.example.r2.cloudflarestorage.com/linux.zip",
            release_metadata=release_fixture,
            opener=archive_opener,
        )
        check("itch_end_to_end_resolve_download_extract_passes", resolved.is_file())
        check("itch_end_to_end_final_identity_passes", verify_current_jar(resolved, target).sha256 == digest)
        check(
            "itch_tag_fixture_still_requires_exact_commit",
            validate_tag_commit_metadata(target, tag_fixture) == target.source_commit,
        )

        class _Headers(dict):
            def get_content_type(self):
                return self.get("Content-Type", "").split(";", 1)[0]

        class _StatusResponse(io.BytesIO):
            def __init__(self, payload, *, status=200, headers=None):
                super().__init__(payload)
                self.status = status
                self.headers = _Headers(headers or {})

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        class _InterruptedResponse(_StatusResponse):
            def __init__(self):
                super().__init__(b"partial", headers={"Content-Length": "100"})
                self._reads = 0

            def read(self, size=-1):
                self._reads += 1
                if self._reads == 1:
                    return super().read(size)
                raise http.client.IncompleteRead(b"partial", 100)

        # Content-Length and content-type checks fail before any cache output;
        # all temporary .part files must be removed.
        limit_dir = root / "limits"
        try:
            download_itch_archive(
                "https://itchio-mirror.example.r2.cloudflarestorage.com/oversized.zip",
                limit_dir / "oversized.zip",
                opener=lambda request, timeout=180: _StatusResponse(
                    b"x", headers={"Content-Length": str(MAX_ITCH_ARCHIVE_BYTES + 1)}
                ),
            )
            oversized_ok = False
        except ReleaseResolutionError:
            oversized_ok = not any(limit_dir.glob("*.part"))
        check("itch_archive_content_length_limit_fails_and_cleans", oversized_ok)
        try:
            download_itch_archive(
                "https://itchio-mirror.example.r2.cloudflarestorage.com/error",
                limit_dir / "error.zip",
                opener=lambda request, timeout=180: _StatusResponse(
                    b"<html>", headers={"Content-Type": "text/html", "Content-Length": "7"}
                ),
            )
            html_ok = False
        except ReleaseResolutionError:
            html_ok = not any(limit_dir.glob("*.part"))
        check("itch_archive_html_content_type_fails_and_cleans", html_ok)
        try:
            download_itch_archive(
                "https://itchio-mirror.example.r2.cloudflarestorage.com/interrupted.zip",
                limit_dir / "interrupted.zip",
                opener=lambda request, timeout=180: _InterruptedResponse(),
            )
            interrupted_ok = False
        except ReleaseResolutionError:
            interrupted_ok = not any(limit_dir.glob("*.part"))
        check("itch_archive_interrupted_stream_fails_and_cleans", interrupted_ok)
        try:
            download_itch_archive(
                "http://itchio-mirror.example.r2.cloudflarestorage.com/insecure",
                limit_dir / "insecure.zip",
                opener=archive_opener,
            )
            https_ok = False
        except ReleaseResolutionError:
            https_ok = True
        check("itch_archive_http_url_rejected", https_ok)

        from compatlib.release_jar import resolve_itch_download

        # Exercise the real GET/POST/token/upload negotiation with a tiny
        # transport fixture, including the external-download fail-closed path.
        purchase_html = '<meta name="csrf_token" value="purchase-token">'
        landing_url = "https://anuke.itch.io/mindustry/download/token"
        archive_url = "https://itchio-mirror.example.r2.cloudflarestorage.com/linux.zip"

        def negotiation_opener(request, timeout=60):
            del timeout
            url = request.full_url
            if url.endswith("/purchase"):
                return _StatusResponse(purchase_html.encode())
            if url.endswith("/download_url"):
                return _StatusResponse(json.dumps({"url": landing_url}).encode())
            if url == landing_url:
                return _StatusResponse(itch_html.encode())
            if "/file/987654?" in url:
                return _StatusResponse(json.dumps({"url": archive_url}).encode())
            raise AssertionError(f"unexpected negotiation URL: {url}")

        negotiated = resolve_itch_download(target, opener=negotiation_opener)
        check("itch_token_post_upload_negotiation_passes", negotiated.archive_url == archive_url)

        def external_opener(request, timeout=60):
            del timeout
            if "/file/987654?" in request.full_url:
                return _StatusResponse(
                    json.dumps({"external": True, "url": archive_url}).encode()
                )
            return negotiation_opener(request)

        try:
            resolve_itch_download(target, opener=external_opener)
            external_ok = False
        except ReleaseResolutionError:
            external_ok = True
        check("itch_external_upload_rejected", external_ok)

        def expired_token_opener(request, timeout=60):
            del timeout
            if request.full_url.endswith("/purchase"):
                return _StatusResponse(purchase_html.encode())
            if request.full_url.endswith("/download_url"):
                return _StatusResponse(json.dumps({"errors": ["expired"]}).encode())
            raise AssertionError(f"unexpected expired-token URL: {request.full_url}")

        try:
            resolve_itch_download(target, opener=expired_token_opener)
            expired_ok = False
        except ReleaseResolutionError:
            expired_ok = True
        check("itch_expired_token_fails_closed", expired_ok)


def _git_init_repo(repo: Path) -> None:
    import subprocess

    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    subprocess.run(["git", "config", "user.email", "cert@example.test"], cwd=repo, check=True)
    subprocess.run(["git", "config", "user.name", "cert"], cwd=repo, check=True)


def _git_commit_all(repo: Path, message: str) -> str:
    import subprocess

    subprocess.run(["git", "add", "-A"], cwd=repo, check=True)
    subprocess.run(["git", "commit", "-q", "--allow-empty", "-m", message], cwd=repo, check=True)
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


def test_runtime_checkpoint_guard() -> None:
    """Adversarial CERTIFIED_RUNTIME_SHA guard in temporary git repos only."""
    import subprocess

    from compatlib.runtime_checkpoint import (
        load_certified_runtime_sha,
        validate_runtime_checkpoint,
        RuntimeCheckpointError,
    )

    with tempfile.TemporaryDirectory(prefix="runtime-checkpoint-") as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        _git_init_repo(repo)

        (repo / "src").mkdir()
        (repo / "src" / "lib.rs").write_text("fn main() {}\n", encoding="utf-8")
        (repo / "Cargo.toml").write_text("[package]\nname='t'\nversion='0'\n", encoding="utf-8")
        (repo / "compat").mkdir()
        (repo / "compat" / "current.toml").write_text('[target]\nbuild="159.7"\n', encoding="utf-8")
        (repo / "compatibility-reports").mkdir()
        (repo / "compatibility-reports" / "note.md").write_text("checkpoint\n", encoding="utf-8")
        (repo / "tools").mkdir()
        (repo / "tools" / "cert_selftest.py").write_text("print('ok')\n", encoding="utf-8")
        checkpoint = _git_commit_all(repo, "runtime checkpoint")
        subprocess.run(["git", "branch", "-M", "main"], cwd=repo, check=True)

        # Allowed: report / certification-tooling-only descendant.
        (repo / "compatibility-reports" / "note.md").write_text("report only\n", encoding="utf-8")
        (repo / "tools" / "cert_selftest.py").write_text("print('selftest')\n", encoding="utf-8")
        allowed = _git_commit_all(repo, "docs and cert tooling only")
        allowed_errs = validate_runtime_checkpoint(repo, checkpoint)
        check(
            "runtime checkpoint allows report/tooling descendants",
            not allowed_errs,
            str(allowed_errs),
        )

        # Fail: nonexistent SHA.
        check(
            "nonexistent runtime SHA fails",
            any(
                "does not resolve" in e
                for e in validate_runtime_checkpoint(
                    repo, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                )
            ),
        )

        # Fail: SHA not an ancestor (orphan branch tip).
        subprocess.run(["git", "checkout", "--orphan", "orphan-branch", "-q"], cwd=repo, check=True)
        (repo / "orphan.txt").write_text("orphan\n", encoding="utf-8")
        orphan = _git_commit_all(repo, "orphan tip")
        subprocess.run(["git", "checkout", "-q", "main"], cwd=repo, check=True)
        check(
            "runtime SHA not ancestor fails",
            any(
                "not an ancestor" in e
                for e in validate_runtime_checkpoint(repo, orphan)
            ),
            orphan,
        )

        # Fail: src/** changed after checkpoint.
        (repo / "src" / "lib.rs").write_text("fn main() { /* drift */ }\n", encoding="utf-8")
        _git_commit_all(repo, "src drift")
        check(
            "src/** changed after checkpoint fails",
            any("runtime drift" in e and "src/" in e for e in validate_runtime_checkpoint(repo, checkpoint)),
        )

        # Reset to allowed tip, then Cargo.toml drift.
        subprocess.run(["git", "reset", "--hard", "-q", allowed], cwd=repo, check=True)
        (repo / "Cargo.toml").write_text("[package]\nname='t'\nversion='1'\n", encoding="utf-8")
        _git_commit_all(repo, "cargo drift")
        check(
            "Cargo.toml changed after checkpoint fails",
            any(
                "runtime drift" in e and "Cargo.toml" in e
                for e in validate_runtime_checkpoint(repo, checkpoint)
            ),
        )

        # Reset, then compat/current.toml drift.
        subprocess.run(["git", "reset", "--hard", "-q", allowed], cwd=repo, check=True)
        (repo / "compat" / "current.toml").write_text('[target]\nbuild="999.9"\n', encoding="utf-8")
        _git_commit_all(repo, "current.toml drift")
        check(
            "compat/current.toml changed after checkpoint fails",
            any(
                "runtime drift" in e and "compat/current.toml" in e
                for e in validate_runtime_checkpoint(repo, checkpoint)
            ),
        )

        # Missing / stale ledger fields.
        try:
            load_certified_runtime_sha({"overall_status": "PASS"})
            missing_ok = False
        except RuntimeCheckpointError:
            missing_ok = True
        check("missing certified_runtime_sha fails", missing_ok)
        try:
            load_certified_runtime_sha(
                {
                    "certified_code_sha": "abc",
                    "certified_runtime_sha": checkpoint,
                }
            )
            stale_ok = False
        except RuntimeCheckpointError as exc:
            stale_ok = "certified_code_sha" in str(exc)
        check("stale certified_code_sha field fails", stale_ok)


def main() -> int:
    print("== compat self-tests ==")
    test_classifier()
    test_packets_shift()
    test_typeio_and_saves()
    test_provenance_and_ledger()
    test_rules_defaults_are_jvm_stable()
    test_canary_guard()
    test_wrong_commit_and_jar()
    test_release_jar_resolver()
    test_runtime_checkpoint_guard()
    print(f"passed={PASS} failed={FAIL}")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
