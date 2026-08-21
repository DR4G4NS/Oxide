#!/usr/bin/env python3
"""Extract/verify Mindustry compatibility manifests (schema v2)."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib.atomic import atomic_write_tree
from compatlib.classifier import fingerprint_source, resolve_commit
from compatlib.current import load_current
from compatlib.jar_identity import check_jar_build, sha256_file
from compatlib.schema import validate_compat_dir
from compatlib.wrap import provenance, wrap

FINGERPRINT_FILES = [
    "core/src/mindustry/net/Net.java",
    "core/src/mindustry/net/Packets.java",
    "core/src/mindustry/net/NetworkIO.java",
    "core/src/mindustry/core/NetServer.java",
    "core/src/mindustry/io/TypeIO.java",
    "core/src/mindustry/io/SaveVersion.java",
    "core/src/mindustry/io/versions/Save12.java",
    "core/src/mindustry/io/versions/Save13.java",
    "core/src/mindustry/ai/ControlPathfinder.java",
    "core/src/mindustry/ai/types/CommandAI.java",
    "core/src/mindustry/ai/types/LogicAI.java",
    "core/src/mindustry/logic/LExecutor.java",
    "core/src/mindustry/logic/LAccess.java",
    "core/src/mindustry/entities/comp/StatusComp.java",
    "core/src/mindustry/entities/comp/UnitComp.java",
    "core/src/mindustry/entities/comp/BuildingComp.java",
    "annotations/src/main/java/mindustry/annotations/entity/EntityProcess.java",
]


def resolve_source_repo(explicit: Path | None) -> Path | None:
    if explicit:
        return explicit
    env = os.environ.get("MINDUSTRY_SOURCE")
    if env:
        return Path(env)
    return None


def extract_from_jar(jar_path: Path) -> dict:
    tools_classes = REPO_ROOT / "target" / "tools-classes"
    tools_classes.mkdir(parents=True, exist_ok=True)
    extractor_src = REPO_ROOT / "tools" / "inspect" / "ExtractManifest.java"
    res = subprocess.run(
        ["javac", "-d", str(tools_classes), "-cp", str(jar_path), str(extractor_src)],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"javac failed for ExtractManifest:\n{res.stderr}")
    res = subprocess.run(
        ["java", "-cp", f"{jar_path}:{tools_classes}", "ExtractManifest", str(jar_path)],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"java ExtractManifest failed:\n{res.stderr}")
    raw_out = res.stdout
    json_start = raw_out.find("{")
    if json_start == -1:
        raise RuntimeError(f"No JSON document found in output:\n{raw_out[:500]}")
    return json.loads(raw_out[json_start:])


def parse_typeio_object_tags(source_text: str) -> list[dict]:
    tags = []
    in_write_object = False
    for line in source_text.splitlines():
        if "writeObject(" in line and "void writeObject" in line:
            in_write_object = True
            continue
        if in_write_object and line.strip().startswith("public ") and "writeObject" not in line:
            break
        if in_write_object and "write.b(" in line:
            tags.append({"line": line.strip()})
    return tags


def generate_manifest(
    build: str,
    jar_path: Path,
    source_ref: str,
    source_commit: str,
    source_repo: Path | None,
    check_only: bool = False,
) -> int:
    if not jar_path.exists():
        print(f"error: JAR file not found at {jar_path}", file=sys.stderr)
        return 1

    with zipfile.ZipFile(jar_path) as z:
        if "version.properties" not in z.namelist():
            print(f"error: version.properties missing in {jar_path}", file=sys.stderr)
            return 1
        v_props = z.read("version.properties").decode("utf-8")

    build_err = check_jar_build(jar_path, build)
    if build_err:
        print(f"error: {build_err}", file=sys.stderr)
        return 2

    data = extract_from_jar(jar_path)
    jar_sha = sha256_file(jar_path)
    jar_size = jar_path.stat().st_size
    jar_name = jar_path.name
    meta = dict(
        build=build,
        source_ref=source_ref or f"v{build}",
        source_commit=source_commit,
        jar_filename=jar_name,
        jar_size=jar_size,
        jar_sha256=jar_sha,
    )

    fingerprints = {}
    typeio_tags = data.get("typeio", {}).get("object_tags") if isinstance(data.get("typeio"), dict) else None
    if source_repo is not None:
        for rel in FINGERPRINT_FILES:
            digest = fingerprint_source(source_repo, source_ref or source_commit, rel)
            if digest:
                fingerprints[rel] = digest
        try:
            typeio_src = subprocess.run(
                ["git", "show", f"{source_ref}:core/src/mindustry/io/TypeIO.java"],
                cwd=source_repo,
                capture_output=True,
                text=True,
                check=False,
            )
            if typeio_src.returncode == 0:
                typeio_tags = parse_typeio_object_tags(typeio_src.stdout)
        except OSError:
            pass

    typeio = data.get("typeio") or {"methods": []}
    if isinstance(typeio, list):
        typeio = {"methods": typeio}
    if typeio_tags is not None:
        typeio["object_tags"] = typeio_tags

    logic = data.get("logic")
    if logic is None:
        logic = {"access": data.get("logic_access") or []}
    elif isinstance(logic, list):
        logic = {"access": logic}

    manifest = provenance(**meta)
    manifest.update(
        {
            "version_properties": v_props,
            "save_versions_count": len(data["save_versions"]),
            "packet_count": len(data["packets"]),
            "blocks_count": len(data["blocks"]),
            "units_count": len(data["units"]),
            "items_count": len(data["items"]),
            "liquids_count": len(data["liquids"]),
            "status_effects_count": len(data["status_effects"]),
            "capabilities": data.get("capabilities") or {},
        }
    )

    files_to_write = {
        "manifest.json": manifest,
        "packets.json": wrap("packets", data["packets"], **meta),
        "streams.json": wrap("streams", data.get("streams") or [], **meta),
        "rpc.json": wrap("rpc", data.get("rpc") or [], **meta),
        "typeio.json": wrap("typeio", typeio, **meta),
        "saves.json": wrap("saves", data["save_versions"], **meta),
        "content.json": wrap(
            "content",
            {
                "items": data["items"],
                "liquids": data["liquids"],
                "weathers": data["weathers"],
                "status_effects": data["status_effects"],
                "units": data["units"],
                "blocks": data["blocks"],
                "unit_commands": data["unit_commands"],
                "unit_stances": data["unit_stances"],
            },
            **meta,
        ),
        "entities.json": wrap("entities", data.get("entities") or [], **meta),
        "entity-sync.json": wrap("entity_sync", data.get("entity_sync") or {}, **meta),
        "rules.json": wrap("rules", data["rules_fields"], **meta),
        "logic.json": wrap("logic", logic, **meta),
        "semantic-fingerprints.json": wrap("fingerprints", fingerprints, **meta),
    }

    compat_dir = REPO_ROOT / "compat" / build
    if check_only:
        mismatches = []
        for name, obj in files_to_write.items():
            p = compat_dir / name
            if not p.exists():
                mismatches.append(f"missing {p.relative_to(REPO_ROOT)}")
                continue
            existing = json.loads(p.read_text())
            # Compare without generator timestamp; canonical JSON of payload keys.
            if name == "manifest.json":
                for k in ("build", "source_ref", "source_commit", "packet_count", "save_versions_count"):
                    if existing.get(k) != obj.get(k):
                        mismatches.append(f"{name} key {k} mismatch")
                if existing.get("jar", {}).get("sha256") != obj.get("jar", {}).get("sha256"):
                    mismatches.append("manifest jar sha256 mismatch")
            else:
                key = name.replace(".json", "").replace("-", "_")
                if name == "entity-sync.json":
                    key = "entity_sync"
                if existing.get(key) != obj.get(key):
                    mismatches.append(f"{name} payload mismatch")
        if mismatches:
            print(f"Manifest check FAILED for build {build}:", file=sys.stderr)
            for m in mismatches:
                print(f"  - {m}", file=sys.stderr)
            return 3
        print(f"Manifest check OK for build {build}")
        return 0

    atomic_write_tree(compat_dir, files_to_write)
    print(f"wrote compat/{build} ({len(files_to_write)} artifacts, sha256={jar_sha})")
    return 0


def main() -> int:
    # Allow `python3 tools/mindustry_manifest.py` by putting tools/ on path.
    parser = argparse.ArgumentParser(description="Extract/Verify Mindustry Compatibility Manifest v2")
    parser.add_argument("--build", help="Target build (e.g. 159.7). Defaults to compat/current.toml")
    parser.add_argument("--jar", type=Path, default=None)
    parser.add_argument("--source-ref", default="")
    parser.add_argument("--source-commit", default="")
    parser.add_argument("--source-repo", type=Path, default=None)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    current = load_current()
    build = args.build or current["target"]["build"]
    source_ref = args.source_ref or (current["target"]["source_tag"] if build == current["target"]["build"] else f"v{build}")
    source_commit = args.source_commit
    source_repo = resolve_source_repo(args.source_repo)
    if not source_commit and source_repo is not None:
        try:
            source_commit = resolve_commit(source_repo, source_ref)
        except Exception as exc:
            print(f"warning: could not resolve {source_ref}: {exc}", file=sys.stderr)
            source_commit = current["target"].get("source_commit", "")
    elif not source_commit:
        source_commit = current["target"].get("source_commit", "") if build == current["target"]["build"] else ""

    if args.jar is None:
        errors = validate_compat_dir(REPO_ROOT / "compat" / build, expected_build=build)
        if errors:
            print(f"Manifest artifacts invalid for build {build}:", file=sys.stderr)
            for e in errors:
                print(f"  - {e}", file=sys.stderr)
            return 3
        print(f"Manifest artifacts verified OK for build {build}")
        return 0

    return generate_manifest(build, args.jar, source_ref, source_commit, source_repo, args.check)


if __name__ == "__main__":
    # Import path: tools/ is sys.path[0] when executed as a script from repo via python3 tools/...
    tools_dir = Path(__file__).resolve().parent
    if str(tools_dir) not in sys.path:
        sys.path.insert(0, str(tools_dir))
    raise SystemExit(main())
