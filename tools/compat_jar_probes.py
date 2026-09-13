#!/usr/bin/env python3
"""Run version-sensitive behavioral probes against the official Build JAR."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools"))

from compatlib.current import current_build, current_jar_sha256, load_current
from compatlib.jar_identity import check_jar_sha256, sha256_file

PATCH_SAVE12_EMPTY = bytes(12)
PATCH_SAVE13_EMPTY = bytes([0, 0, 0, 2, 0, 0, 0, 0])
FIXTURE_DIR = REPO_ROOT / "tests" / "fixtures" / "official-159.7"
PROBE_SRC = REPO_ROOT / "tools" / "inspect" / "ProbeSave159.java"
TOOLS_CLASSES = REPO_ROOT / "target" / "tools-classes"


def fail(msg: str) -> None:
    print(f"JAR probe FAILED: {msg}", file=sys.stderr)
    raise SystemExit(1)


def compile_probe(jar: Path) -> None:
    TOOLS_CLASSES.mkdir(parents=True, exist_ok=True)
    res = subprocess.run(
        ["javac", "-d", str(TOOLS_CLASSES), "-cp", str(jar), str(PROBE_SRC),
         str(REPO_ROOT / "tools/inspect/InspectBulletCollision.java"),
         str(REPO_ROOT / "tools/inspect/InspectUnitProduction.java"),
         str(REPO_ROOT / "tools/inspect/CheckUnitReload.java"),
         str(REPO_ROOT / "tools/inspect/CheckReconstructorSnapshots.java"),
         str(REPO_ROOT / "tools/inspect/CheckSupportSnapshots.java")],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        fail(f"javac ProbeSave159:\n{res.stderr}")


def run_probe(jar: Path, *args: str) -> None:
    res = subprocess.run(
        ["java", "-cp", f"{jar}:{TOOLS_CLASSES}", "ProbeSave159", *args],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        fail(f"ProbeSave159 {' '.join(args)}:\n{res.stdout}\n{res.stderr}")
    print(res.stdout.strip())


def main() -> int:
    doc = load_current()
    build = current_build(doc)
    want_sha = current_jar_sha256(doc)
    jar = os.environ.get("MINDUSTRY_CURRENT_JAR") or os.environ.get("MINDUSTRY_DESKTOP_JAR")
    if not jar:
        fail("Set MINDUSTRY_CURRENT_JAR")
    jar_path = Path(jar)
    if not jar_path.exists():
        fail(f"JAR not found: {jar}")
    sha_err = check_jar_sha256(jar_path, want_sha)
    if sha_err:
        fail(sha_err)
    got = sha256_file(jar_path)
    print(f"== JAR behavioral probes Build {build} sha={got[:12]}... ==")
    compile_probe(jar_path)
    collision = subprocess.run(
        ["java", "-cp", f"{jar_path}:{TOOLS_CLASSES}", "InspectBulletCollision"],
        capture_output=True, text=True,
    )
    expected = (REPO_ROOT / "src/game/bullet_collision.tsv").read_text(encoding="utf-8")
    rows = lambda text: [line for line in text.splitlines() if line and line[0].isdigit()]
    if collision.returncode or rows(collision.stdout) != rows(expected):
        fail(f"BulletType collision table differs from initialized target JAR:\n{collision.stderr}")
    print("OK initialized BulletType collision table")
    production = subprocess.run(
        ["java", "-cp", f"{jar_path}:{TOOLS_CLASSES}", "InspectUnitProduction"],
        capture_output=True, text=True,
    )
    expected = (REPO_ROOT / "src/game/unit_production.tsv").read_text(encoding="utf-8")
    if production.returncode or rows(production.stdout) != rows(expected):
        fail(f"Unit production recipes differ from initialized target JAR:\n{production.stderr}")
    print("OK initialized unit production recipes and input capacities")
    reload_check = subprocess.run(
        ["java", "-cp", f"{jar_path}:{TOOLS_CLASSES}", "CheckUnitReload"],
        capture_output=True, text=True,
    )
    if reload_check.returncode or "OK initialized unit reloads mounts=20" not in reload_check.stdout:
        fail(f"Unit reload probe failed:\n{reload_check.stdout}\n{reload_check.stderr}")
    print(reload_check.stdout.strip())

    with tempfile.TemporaryDirectory(prefix="jar-probes-") as tmp:
        tmp_path = Path(tmp)
        fixture_env = dict(os.environ, OXIDE_RECONSTRUCTOR_FIXTURE_DIR=str(tmp_path))
        for test in ["survival_conveyor_delivery_reconstructor_consumes_power_from_actual_payload",
                     "f03_unit_factory_plan_minus_one_deselects"]:
            fixtures = subprocess.run(
                ["cargo", "test", "--lib", test, "--", "--test-threads=1"],
                cwd=REPO_ROOT, env=fixture_env, capture_output=True, text=True,
            )
            if fixtures.returncode or "1 passed; 0 failed" not in fixtures.stdout:
                fail(f"Production fixture generation failed:\n{fixtures.stdout}\n{fixtures.stderr}")
        snapshots = subprocess.run(
            ["java", "-cp", f"{jar_path}:{TOOLS_CLASSES}", "CheckReconstructorSnapshots", str(tmp_path)],
            capture_output=True, text=True,
        )
        if snapshots.returncode or "OK reconstructor complete snapshots" not in snapshots.stdout:
            fail(f"Reconstructor snapshot read failed:\n{snapshots.stdout}\n{snapshots.stderr}")
        print(snapshots.stdout.strip())
        support_env = dict(os.environ, OXIDE_SUPPORT_FIXTURES=str(tmp_path))
        for test in ["plastanium_line_fed_by_multitile_drill_survives_inspection",
                     "support_unit_weapons_aim_only_at_real_targets_and_heal_on_impact"]:
            fixtures = subprocess.run(
                ["cargo", "test", "--lib", test, "--", "--test-threads=1"],
                cwd=REPO_ROOT, env=support_env, capture_output=True, text=True,
            )
            if fixtures.returncode or "1 passed; 0 failed" not in fixtures.stdout:
                fail(f"Support fixture generation failed:\n{fixtures.stdout}\n{fixtures.stderr}")
        support = subprocess.run(
            ["java", "-cp", f"{jar_path}:{TOOLS_CLASSES}", "CheckSupportSnapshots", str(tmp_path)],
            capture_output=True, text=True,
        )
        if support.returncode or "OK plastanium drill input and actual draw" not in support.stdout:
            fail(f"Support snapshot probe failed:\n{support.stdout}\n{support.stderr}")
        print(support.stdout.strip())
        save12_patch = tmp_path / "save12-empty.patch"
        save13_patch = tmp_path / "save13-empty.patch"
        save12_patch.write_bytes(PATCH_SAVE12_EMPTY)
        save13_patch.write_bytes(PATCH_SAVE13_EMPTY)
        run_probe(jar_path, "read-save12-patches", str(save12_patch))
        run_probe(jar_path, "reject-save13-patches-on-save12", str(save13_patch))

    save13 = FIXTURE_DIR / "official-save13-empty.msav"
    save12 = FIXTURE_DIR / "official-save12-empty.msav"
    if save13.exists():
        run_probe(jar_path, "read-msav-meta", str(save13))
    else:
        fail(f"missing fixture {save13}")
    if save12.exists():
        run_probe(jar_path, "read-msav-meta", str(save12))
    else:
        fail(f"missing fixture {save12}")

    # SpawnUnitI: team + effect are LVar runtime operands in official bytecode
    res = subprocess.run(
        ["javap", "-classpath", str(jar_path), "-private", "-c", "mindustry.logic.LExecutor$SpawnUnitI"],
        capture_output=True,
        text=True,
    )
    out = res.stdout
    if res.returncode != 0 or "team" not in out:
        fail("javap SpawnUnitI missing team field/method")
    if "effect" not in out:
        fail("javap SpawnUnitI missing effect field")
    # Official run() gates statuses with effect.bool() on the LVar operand.
    if "bool" not in out:
        fail("javap SpawnUnitI missing effect.bool() runtime evaluation")
    print("OK javap SpawnUnitI team+effect runtime LVar operands present")

    # bestCore ranking uses CoreBlock.size
    res = subprocess.run(
        ["javap", "-classpath", str(jar_path), "-c", "mindustry.gen.Player"],
        capture_output=True,
        text=True,
    )
    if "bestCore" not in res.stdout:
        fail("javap PlayerComp.bestCore missing")
    if "comparingInt" not in res.stdout and "dst2" not in res.stdout:
        fail("javap PlayerComp.bestCore missing size/distance ranking")
    print("OK javap PlayerComp.bestCore ranking present")

    print(f"== JAR behavioral probes PASSED for Build {build} ==")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
