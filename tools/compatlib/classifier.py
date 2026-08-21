"""Discover and classify upstream source deltas. Discovery is not semantic proof."""

from __future__ import annotations

import hashlib
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

CATEGORIES = (
    "WIRE_PACKET",
    "STREAMING",
    "RPC",
    "TYPEIO",
    "SAVE",
    "MAP",
    "CONTENT",
    "ENTITY_SYNC",
    "RULES",
    "PLACEMENT",
    "LOGIC",
    "STATUS",
    "AI",
    "UNITS",
    "PHYSICS_COLLISION",
    "COMBAT",
    "ECONOMY",
    "STATEFUL_BUILDING",
    "INPUT_AUTHORITY",
    "ADMIN",
    "CLIENT_ONLY",
    "EDITOR_ONLY",
    "UNKNOWN_REQUIRES_PROBE",
)

# First matching rule wins.
PATH_RULES: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"/editor/|EditorMaps|MapEditor|MapFixer"), "EDITOR_ONLY"),
    (re.compile(
        r"/ui/|/graphics/|/audio/|DesktopLauncher|ClientLauncher|AndroidLauncher|"
        r"Styles\.java|Dialogs\.java|/steam/|Renderer\.java$|/core/UI\.java$|"
        r"/core/Control\.java$|LCanvas\.java$|LogicDialog\.java$|"
        r"DesktopInput\.java$|Binding\.java$|ios/"
    ), "CLIENT_ONLY"),
    (re.compile(r"Net\.java$|Packets\.java$|ArcNetProvider"), "WIRE_PACKET"),
    (re.compile(r"Streamable\.java$|NetworkIO\.java$"), "STREAMING"),
    (re.compile(r"TypeIO\.java$"), "TYPEIO"),
    (re.compile(r"SaveIO\.java$|SaveVersion\.java$|SaveFileReader\.java$|Save\d+\.java$|SaveMeta\.java$|SaveOptions\.java$|SaveReadState\.java$"), "SAVE"),
    (re.compile(r"MapIO\.java$|/mod/data/|DataPatcher\.java$|DataManager\.java$|DataAsset"), "MAP"),
    (re.compile(r"EntityProcess\.java$|GroupDefs\.java$|EntityGroup\.java$|SyncComp\.java$"), "ENTITY_SYNC"),
    (re.compile(r"BuildingComp\.java$|UnitComp\.java$|EmptyDataAbility\.java$"), "ENTITY_SYNC"),
    (re.compile(r"Rules\.java$|CampaignRules\.java$|Team\.java$|Teams\.java$"), "RULES"),
    (re.compile(r"/core/Logic\.java$"), "ENTITY_SYNC"),
    (re.compile(r"BlockIndexer\.java$"), "AI"),
    (re.compile(r"/core/World\.java$"), "MAP"),
    (re.compile(r"ContentLoader\.java$|/ctype/"), "CONTENT"),
    (re.compile(r"WorldLabelComp\.java$"), "ENTITY_SYNC"),
    (re.compile(r"EventType\.java$"), "ADMIN"),
    (re.compile(r"/game/Saves\.java$|Schematics\.java$"), "SAVE"),
    (re.compile(r"annotations/.*/EntityProcess\.java$"), "ENTITY_SYNC"),
    (re.compile(r"annotations/"), "CLIENT_ONLY"),
    (re.compile(r"^tests/|^android/|^ios/"), "CLIENT_ONLY"),
    (re.compile(r"ConstructBlock\.java$|/world/Build\.java$|Block\.java$"), "PLACEMENT"),
    (re.compile(r"LExecutor\.java$|LStatements\.java$|LAccess\.java$|LogicRule\.java$|GlobalVars\.java$|LogicScript\.java$|LogicBlock\.java$"), "LOGIC"),
    (re.compile(r"StatusComp\.java$|StatusEffect\.java$"), "STATUS"),
    (re.compile(r"ControlPathfinder\.java$|Pathfinder\.java$|CommandAI\.java$|LogicAI\.java$|RtsAI\.java$"), "AI"),
    (re.compile(r"UnitType\.java$|/entities/Units\.java$|UnitFactory\.java$|Reconstructor\.java$|UnitAssembler\.java$"), "UNITS"),
    (re.compile(r"PhysicsProcess\.java$|EntityCollisions\.java$"), "PHYSICS_COLLISION"),
    (re.compile(r"Damage\.java$|BulletType\.java$|/turrets/|ShockwaveTower\.java$"), "COMBAT"),
    (re.compile(r"PayloadConveyor\.java$|Unloader\.java$|Router\.java$|NuclearReactor\.java$|ItemModule\.java$|LiquidModule\.java$"), "STATEFUL_BUILDING"),
    (re.compile(r"InputHandler\.java$"), "INPUT_AUTHORITY"),
    (re.compile(r"NetServer\.java$|NetClient\.java$|NetConnection\.java$|Administration\.java$"), "ADMIN"),
    (re.compile(r"JsonIO\.java$"), "SAVE"),
    (re.compile(r"/world/draw/|NoiseEffect\.java$|DrawPart\.java$"), "CLIENT_ONLY"),
    (re.compile(r"/maps/"), "MAP"),
    (re.compile(r"/mod/Data"), "MAP"),
    (re.compile(r"/mod/"), "CLIENT_ONLY"),
    (re.compile(r"ServerControl\.java$"), "ADMIN"),
    (re.compile(r"/world/Tile\.java$"), "PLACEMENT"),
    (re.compile(r"PowerGraph\.java$"), "STATEFUL_BUILDING"),
    (re.compile(r"/world/blocks/"), "STATEFUL_BUILDING"),
    (re.compile(r"Item\.java$|Planet\.java$|Weather\.java$"), "CONTENT"),
    (re.compile(r"Vars\.java$|GameState\.java$"), "RULES"),
    (re.compile(r"FileTree\.java$|Platform\.java$|PerfCounter\.java$|GameService\.java$"), "CLIENT_ONLY"),
    (re.compile(r"CrashHandler\.java$|SteamAdmin\.java$|WorldReloader\.java$"), "ADMIN"),
    (re.compile(r"tools/src/"), "CLIENT_ONLY"),
]

AUTHORITATIVE_HINTS = re.compile(
    r"@Remote|writeSync|readSync|write\(|read\(|updateTile|updateUnit|handleServer"
)


@dataclass
class FileDelta:
    path: str
    status: str  # A/M/D
    category: str
    symbols: list[str] = field(default_factory=list)
    notes: str = ""


def run_git(source_repo: Path, args: list[str]) -> str:
    res = subprocess.run(
        ["git", *args],
        cwd=source_repo,
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {res.stderr.strip()}")
    return res.stdout


def resolve_commit(source_repo: Path, ref: str) -> str:
    return run_git(source_repo, ["rev-parse", f"{ref}^{{commit}}"]).strip()


def classify_path(path: str) -> str:
    posix = path.replace("\\", "/")
    if posix.endswith((".png", ".jpg", ".ogg", ".mp3", ".wav", ".md", ".properties", ".json")):
        if "/editor" in posix:
            return "EDITOR_ONLY"
        return "CLIENT_ONLY"
    if "/assets/maps/" in posix or posix.endswith(".msav"):
        return "MAP"
    for pattern, category in PATH_RULES:
        if pattern.search(posix):
            return category
    if posix.endswith(".java") and "mindustry" in posix and not posix.startswith("tests/"):
        return "UNKNOWN_REQUIRES_PROBE"
    if posix.endswith((".java", ".gradle", ".js")) and "core/src/mindustry" in posix:
        return "UNKNOWN_REQUIRES_PROBE"
    return "CLIENT_ONLY"


def parse_symbols(diff_text: str, limit: int = 24) -> list[str]:
    symbols: list[str] = []
    for match in re.finditer(
        r"^(?:public |protected |private |static |final |synchronized |native )*(?:class|interface|enum|void|[A-Z@][\w.<>\[\]]+)\s+(\w+)\s*\(",
        diff_text,
        re.M,
    ):
        name = match.group(1)
        if name not in symbols:
            symbols.append(name)
        if len(symbols) >= limit:
            break
    for match in re.finditer(r"^@@ .* @@(?:\s+(.*))?$", diff_text, re.M):
        ctx = (match.group(1) or "").strip()
        if ctx and ctx not in symbols:
            symbols.append(ctx[:120])
        if len(symbols) >= limit:
            break
    return symbols


def enumerate_delta(source_repo: Path, from_ref: str, to_ref: str) -> list[FileDelta]:
    from_sha = resolve_commit(source_repo, from_ref)
    to_sha = resolve_commit(source_repo, to_ref)
    name_status = run_git(source_repo, ["diff", "--name-status", f"{from_sha}..{to_sha}"])
    rows: list[FileDelta] = []
    for line in name_status.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        status, path = parts[0][0], parts[-1]
        category = classify_path(path)
        symbols: list[str] = []
        notes = ""
        if path.endswith(".java") and status != "D":
            try:
                snippet = run_git(
                    source_repo,
                    ["diff", "-U0", f"{from_sha}..{to_sha}", "--", path],
                )
                symbols = parse_symbols(snippet)
                if category == "UNKNOWN_REQUIRES_PROBE" and AUTHORITATIVE_HINTS.search(snippet):
                    notes = "changed file has potentially authoritative symbols"
            except RuntimeError:
                pass
        rows.append(
            FileDelta(path=path, status=status, category=category, symbols=symbols, notes=notes)
        )
    return rows


def fingerprint_source(source_repo: Path, ref: str, relpath: str) -> str | None:
    try:
        blob = run_git(source_repo, ["show", f"{ref}:{relpath}"])
    except RuntimeError:
        return None
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()
