#!/usr/bin/env python3
"""Generate compat/159.7/certification-ledger.json from source delta + curated statuses."""

from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compatlib.atomic import canonical_dumps
from compatlib.classifier import enumerate_delta
from compatlib.current import load_current

REPO = Path(__file__).resolve().parent.parent


def row(**kwargs):
    base = {
        "upstream_files": [],
        "upstream_symbols": [],
        "source_change": "",
        "rust_owner": "",
        "risk": "MEDIUM",
        "implementation_required": False,
        "evidence_required": True,
        "status": "EVIDENCE_REQUIRED",
        "source_evidence": "",
        "jar_probe": "",
        "rust_tests": "",
        "notes": "",
    }
    base.update(kwargs)
    return base


def main() -> int:
    current = load_current()
    import os
    source = Path(os.environ.get("MINDUSTRY_SOURCE", "")).expanduser()
    if not source.exists():
        print("error: set MINDUSTRY_SOURCE to the official Mindustry git checkout", file=sys.stderr)
        return 2
    files = enumerate_delta(source, "v158.1", "v159.7")
    by_cat = defaultdict(list)
    for f in files:
        by_cat[f.category].append(f.path)

    curated = [
        row(
            id="WIRE-001",
            category="WIRE_PACKET",
            upstream_files=["core/src/mindustry/net/Net.java", "core/src/mindustry/net/Packets.java"],
            source_change="6 stream/handshake packets; 159 CallPackets; total 165",
            rust_owner="src/network/protocol.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            source_evidence="git diff v158.1..v159.7 -- core/src/mindustry/net/Net.java",
            jar_probe="compat/159.7/packets.json packet_count=165",
            rust_tests="rust_packet_ids_match_committed_159_7_packets_json, generated_packet_ids_match_exact_desktop_159_registry",
        ),
        row(
            id="JOIN-001",
            category="STREAMING",
            upstream_files=["core/src/mindustry/net/NetworkIO.java", "core/src/mindustry/core/NetServer.java"],
            source_change="NetworkIO.writeWorld prefixes writeDataPatches; vanilla maps skip AssetRequirementStream",
            rust_owner="src/engine/world_stream.rs, src/network/session/mod.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            source_evidence="v159.7 NetworkIO.writeWorld writeDataPatches then rules",
            rust_tests="current_personalize_keeps_159_7_data_patch_prefix",
            notes="Vanilla scope: empty data patches only. External assets rejected at map load.",
        ),
        row(
            id="JOIN-002",
            category="STREAMING",
            upstream_files=["core/src/mindustry/core/NetServer.java"],
            source_change="AssetRequirementStream / RequestAssets / AssetStream / RequestWorld FSM",
            rust_owner="src/engine/world_stream.rs",
            risk="HIGH",
            implementation_required=False,
            status="OUT_OF_SCOPE_EXPLICIT",
            source_evidence="sendWorldAndAssets only if hasExternalAssets",
            notes="Certified vanilla scope rejects maps with non-empty data patches or external assets rather than implementing the asset join FSM.",
        ),
        row(
            id="SAVE-013",
            category="SAVE",
            upstream_files=["core/src/mindustry/io/versions/Save13.java", "core/src/mindustry/io/SaveVersion.java"],
            source_change="Save13 is current writer; empty writeDataPatches is 8 bytes",
            rust_owner="src/engine/save_io.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            rust_tests="official_159_7_save13_fixture_reads, write_msav_complete_emits_official_region_order_per_version",
        ),
        row(
            id="SAVE-012",
            category="SAVE",
            upstream_files=["core/src/mindustry/io/versions/Save12.java"],
            source_change="Save12.readDataPatches is transitional (patchAmount+imageAmount), not Save13 format",
            rust_owner="src/engine/save_io.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            rust_tests="rust_save12_empty_patches_round_trip_read_map",
            notes="Official 159.7 never writes Save12. Empty reader fixture is 12 zero bytes after version int.",
        ),
        row(
            id="LOGIC-FLY",
            category="LOGIC",
            upstream_files=["core/src/mindustry/logic/LAccess.java"],
            source_change="@flying ordinal 58",
            rust_owner="src/logic/ops.rs, src/logic/view.rs",
            risk="MEDIUM",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            rust_tests="test_logic_sensor_flying",
        ),
        row(
            id="LOGIC-SPAWN",
            category="LOGIC",
            upstream_files=[
                "core/src/mindustry/logic/LExecutor.java",
                "core/src/mindustry/entities/Units.java",
                "core/src/mindustry/type/UnitType.java",
                "core/src/mindustry/type/unit/MissileUnitType.java",
                "core/src/mindustry/content/UnitTypes.java",
                "core/src/mindustry/world/blocks/defense/BuildTurret.java",
            ],
            source_change="SpawnUnitI object-only UnitType gate; team/effect LVars; World.unconv; Mathf.range(0.01f); exact Units.canCreate useUnitCap short-circuit",
            rust_owner="src/logic/executor.rs, src/logic/view.rs, src/logic/compiler.rs, src/network/economy/factories.rs, src/game/unit_types.rs",
            risk="MEDIUM",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            source_evidence="v159.7 LExecutor.java:1610 type.obj() instanceof UnitType && !type.internal && Units.canCreate; Units.java:115-116 useUnitCap short-circuit; UnitType.java:205/239 defaults; UnitTypes.java:4565 block internal; BuildTurret.java:56-58 generated internal turret type; MissileUnitType.java:20; UnitTypes.java:4612; source c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c; JAR ce1db5b06fe7326b9d0c1d99b1eb1667cf6f0bf97093293f6674ae294981ff05",
            rust_tests="logic_spawn_runtime_unit_type_variable,logic_spawn_direct_numeric_literal_is_invalid,logic_spawn_at_numeric_literal_is_invalid,logic_spawn_bare_name_is_invalid,logic_spawn_numeric_unit_type_is_invalid,logic_spawn_string_unit_type_is_invalid,logic_spawn_quoted_string_literal_is_invalid,logic_spawn_null_unit_type_is_invalid,logic_spawn_non_unittype_object_is_invalid,can_create_unit_under_cap_true,can_create_unit_at_cap_false,can_create_banned_unit_false,can_create_use_unit_cap_false_ignores_cap,can_create_use_unit_cap_false_ignores_ban,unit_type_use_unit_cap_matches_v1597_metadata",
            notes="False-cap vanilla types are non-internal but unsupported by current Rust enemy_spec; direct canCreate regressions cover their canonical metadata without claiming unsupported Spawn integration.",
        ),
        row(
            id="LOGIC-MUSIC",
            category="LOGIC",
            upstream_files=["core/src/mindustry/logic/LExecutor.java"],
            source_change="PlayMusicI is headless no-op",
            rust_owner="n/a",
            risk="LOW",
            status="CLIENT_ONLY",
            source_evidence="PlayMusicI returns if headless",
        ),
        row(
            id="AI-PATH",
            category="AI",
            upstream_files=[
                "core/src/mindustry/ai/ControlPathfinder.java",
                "core/src/mindustry/ai/types/CommandAI.java",
                "core/src/mindustry/ai/types/LogicAI.java",
            ],
            source_change="PathfindResult {move,dest,next,unreachable}; naval next/canPass",
            rust_owner="src/network/units/unit_orders.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            source_evidence="git show v159.7:core/src/mindustry/ai/types/CommandAI.java PathfindResult",
        ),
        row(
            id="RULES-LIMITS",
            category="PLACEMENT",
            upstream_files=["core/src/mindustry/world/blocks/ConstructBlock.java", "core/src/mindustry/game/Rules.java"],
            source_change="blockLimits on place and construct finish",
            rust_owner="src/network/buildings/construction.rs",
            risk="HIGH",
            implementation_required=True,
            status="VERIFIED_IMPLEMENTED",
            rust_tests="test_block_placement_limits; finish_pending_build re-checks limits",
        ),
        row(
            id="STATUS-001",
            category="STATUS",
            upstream_files=["core/src/mindustry/entities/comp/StatusComp.java"],
            source_change="none between v158.1 and v159.7",
            rust_owner="src/network/units/status.rs",
            risk="LOW",
            status="VERIFIED_UNCHANGED",
            source_evidence="git diff v158.1..v159.7 -- core/src/mindustry/entities/comp/StatusComp.java is empty",
        ),
        row(
            id="INPUT-PAYLOAD",
            category="INPUT_AUTHORITY",
            upstream_files=["core/src/mindustry/input/InputHandler.java"],
            source_change="payload pickup/drop methods unchanged; possessionAllowed added",
            rust_owner="src/network/decoders.rs",
            risk="MEDIUM",
            status="REOPENED",
            implementation_required=True,
            source_evidence="git diff v158.1..v159.7 -- core/src/mindustry/input/InputHandler.java",
            notes="Payload framing unchanged. possessionAllowed still needs a dedicated probe.",
        ),
        row(
            id="COMBAT-DELTA",
            category="COMBAT",
            upstream_files=by_cat.get("COMBAT", []),
            source_change="Turret isShooting latch, Damage shield timing, healing homing sourceTeam",
            rust_owner="src/network/combat/",
            risk="HIGH",
            implementation_required=True,
            status="REOPENED",
            source_evidence="git diff v158.1..v159.7 combat files",
        ),
        row(
            id="ENTITY-TICK",
            category="ENTITY_SYNC",
            upstream_files=by_cat.get("ENTITY_SYNC", []),
            source_change="Logic.updateEntities split unit/build/bullet groups",
            rust_owner="src/network/simulation/mod.rs",
            risk="HIGH",
            implementation_required=True,
            status="REOPENED",
            source_evidence="v159.7 GroupDefs / Logic.java updateEntities",
        ),
        row(
            id="STATEFUL-DELTA",
            category="STATEFUL_BUILDING",
            upstream_files=by_cat.get("STATEFUL_BUILDING", []),
            source_change="distribution/power/payload building update diffs",
            rust_owner="src/network/economy/",
            risk="MEDIUM",
            implementation_required=True,
            status="REOPENED",
            source_evidence="v158.1..v159.7 world/blocks",
        ),
        row(
            id="CLIENT-UI",
            category="CLIENT_ONLY",
            upstream_files=by_cat.get("CLIENT_ONLY", [])[:40],
            source_change="UI/graphics/audio/desktop",
            status="CLIENT_ONLY",
            evidence_required=False,
        ),
        row(
            id="EDITOR-UI",
            category="EDITOR_ONLY",
            upstream_files=by_cat.get("EDITOR_ONLY", []),
            source_change="editor dialogs",
            status="EDITOR_ONLY",
            evidence_required=False,
        ),
        row(
            id="UNKNOWN-REMAINING",
            category="UNKNOWN_REQUIRES_PROBE",
            upstream_files=by_cat.get("UNKNOWN_REQUIRES_PROBE", []),
            source_change="changed files not covered by classifier rules",
            risk="HIGH",
            implementation_required=True,
            status="UNKNOWN_REQUIRES_PROBE",
            notes="Fail-closed: any remaining unknown Java file blocks certification.",
        ),
    ]

    # Coverage rows for remaining classified categories
    for cat, rid, owner, status in [
        ("RULES", "RULES-FIELDS", "src/network/units/rules.rs", "VERIFIED_IMPLEMENTED"),
        ("UNITS", "UNITS-TYPE", "src/game/unit_types.rs", "EVIDENCE_REQUIRED"),
        ("CONTENT", "CONTENT-REG", "src/game/", "EVIDENCE_REQUIRED"),
        ("MAP", "MAP-ASSETS", "src/engine/world_stream.rs", "OUT_OF_SCOPE_EXPLICIT"),
        ("ADMIN", "ADMIN-NET", "src/network/session/mod.rs", "EVIDENCE_REQUIRED"),
        ("TYPEIO", "TYPEIO-001", "src/engine/typeio.rs", "VERIFIED_IMPLEMENTED"),
        ("RPC", "RPC-SHIFT", "src/network/protocol.rs", "VERIFIED_IMPLEMENTED"),
        ("PHYSICS_COLLISION", "PHYS-001", "src/network/simulation/", "UNKNOWN_REQUIRES_PROBE"),
    ]:
        paths = by_cat.get(cat, [])
        if not paths:
            continue
        if any(r["id"] == rid for r in curated):
            continue
        curated.append(
            row(
                id=rid,
                category=cat,
                upstream_files=paths,
                rust_owner=owner,
                status=status,
                implementation_required=status not in {"CLIENT_ONLY", "EDITOR_ONLY", "OUT_OF_SCOPE_EXPLICIT", "VERIFIED_UNCHANGED", "VERIFIED_IMPLEMENTED"},
                notes="Category coverage from source-delta engine.",
            )
        )

    doc = {
        "schema_version": 2,
        "build": current["target"]["build"],
        "baseline": "158.1",
        "source_ref": current["target"]["source_tag"],
        "source_commit": current["target"]["source_commit"],
        "migration_baseline": "49d6fe4026cefc153ee6f1e8daac6bc814561700",
        "overall_status": "CERTIFICATION_REOPENED",
        "scope": "vanilla Mindustry v8 Build 159.7 server-authoritative compatibility; no mods; no multi-build runtime; master not certified",
        "rows": curated,
        "classified_file_counts": {k: len(v) for k, v in sorted(by_cat.items())},
    }
    out = REPO / "compat" / "159.7" / "certification-ledger.json"
    out.write_text(canonical_dumps(doc), encoding="utf-8")
    print(f"wrote {out} rows={len(curated)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
