"""Capability-based reverse-dependency scanner.

Diagnostic codes:
  ARCH001 direct domain -> listener
  ARCH002 domain -> runtime / console / TUI
  ARCH003 domain exposes NETWORK_CONNECTION
  ARCH004 domain -> outbound delivery (direct or one-hop facade)
  ARCH005 wire/protocol layer depends on listener implementation
  ARCH006 forbidden dependency via re-export / alias
  ARCH900 unresolved architectural dependency/call

Stdout: TSV rows (no header). Stderr trailer ARCHGUARD_SCAN_COMPLETE is
required so a crash cannot look like an empty-success ledger.
"""
from __future__ import annotations

import os
import re
import sys
from collections import defaultdict
from dataclasses import dataclass, field

from parseutil import (
    extract_functions,
    extract_impl_types,
    extract_structs,
    extract_type_aliases,
    iter_use_statements,
    join_path,
    module_path_for,
    parse_use_tree,
    split_top_level,
    strip_block_comments_and_strings,
    mask_cfg_test_modules,
)

RULE_CLASSES = (
    "ARCH001",
    "ARCH002",
    "ARCH003",
    "ARCH004",
    "ARCH005",
    "ARCH006",
    "ARCH900",
)

CAP_LISTENER = "LISTENER_IMPLEMENTATION"
CAP_RUNTIME = "RUNTIME_CONTROL"
CAP_CONNECTION = "NETWORK_CONNECTION"
CAP_OUTBOUND = "OUTBOUND_DELIVERY"

DOMAIN_PREFIXES = (
    "src/network/economy/",
    "src/network/units/",
    "src/network/combat/",
    "src/network/buildings/",
    "src/logic/",
    "src/network/simulation/",
    "src/network/core_inventory",
)

# World-tick coordinator is orchestration, not domain. Phase bodies stay domain.
ORCHESTRATION_FILES = frozenset({"src/network/simulation/mod.rs"})

# Closed set. A new suppression needs a code change AND a TSV row.
ALLOWED_EXCEPTION_PATHS = frozenset()
ALLOWED_EXCEPTION_CODES = frozenset(RULE_CLASSES)

LEDGER_COLUMNS = (
    "code",
    "source",
    "line",
    "target",
    "capability",
    "chain",
    "owning_phase",
    "evidence",
)

# Free functions that own sockets / connection routing. Not FrameEmit methods.
DELIVERY_FNS = frozenset(
    {
        "crate::network::outbound::broadcast",
        "crate::network::outbound::broadcast_except",
        "crate::network::outbound::enqueue_outbound",
        "crate::network::outbound::enqueue_outbound_routed",
        "crate::network::session::replay::send_generated_packet",
        "crate::network::session::replay::send_generated_packet_prefer_udp",
        "crate::network::listener::broadcast",
        "crate::network::listener::broadcast_except",
        "crate::network::listener::enqueue_outbound",
        "crate::network::listener::enqueue_outbound_routed",
        "crate::network::listener::send_generated_packet",
    }
)
DELIVERY_NAMES = frozenset(
    {
        "enqueue_outbound",
        "enqueue_outbound_routed",
        "send_generated_packet",
        "send_generated_packet_prefer_udp",
        "broadcast_except",
    }
)

RE_PENDING = re.compile(r"\bPendingConnection\b")
RE_REGISTRY = re.compile(
    r"(?:(?:std::sync::)?Arc\s*<\s*)?(?:dashmap::)?DashMap\s*<\s*i32\s*,\s*(?:[\w:]+::)?PendingConnection\s*>"
)
RE_SOCKET = re.compile(r"\b(?:OwnedWriteHalf|TcpStream)\b")
RE_FRAME_EMIT = re.compile(
    r"\bFrameEmit\b|\bNoopEmit\b|\bNOOP\b|dyn\s+(?:crate::network::outbound::)?FrameEmit"
)

HINTS = {
    "ARCH001": "re-point domain onto owning modules; do not import listener",
    "ARCH002": "domain must not depend on runtime/console/TUI",
    "ARCH003": "accept &dyn FrameEmit (or domain state) instead of the connection registry",
    "ARCH004": "emit through &dyn FrameEmit; do not call delivery helpers",
    "ARCH005": "wire may encode; socket routing stays in outbound/listener adapters",
    "ARCH006": "do not hide listener/outbound behind aliases or re-exports",
    "ARCH900": "qualify the symbol so the guard can classify it; do not glob forbidden adapters",
}


@dataclass
class Diagnostic:
    code: str
    source: str
    line: int
    target: str
    capability: str
    chain: str
    owning_phase: str = ""
    evidence: str = ""
    hint: str = ""

    def tsv(self) -> str:
        ev = self.evidence or f"{self.source}:{self.line}"
        hint = self.hint or HINTS.get(self.code, "")
        evidence = f"{ev} | {hint}" if hint else ev
        return "\t".join(
            [
                self.code,
                self.source,
                str(self.line),
                self.target,
                self.capability,
                self.chain,
                self.owning_phase,
                evidence,
            ]
        )


@dataclass
class Import:
    local: str
    path: str
    is_glob: bool
    is_reexport: bool
    line: int


@dataclass
class FnInfo:
    name: str
    line: int
    params: str
    body: str
    module: str
    file: str
    caps: set[str] = field(default_factory=set)
    callees: list[tuple[str, int]] = field(default_factory=list)
    delivery_mentions: list[str] = field(default_factory=list)


@dataclass
class FileInfo:
    path: str
    module: str
    layer: str
    src: str
    imports: list[Import] = field(default_factory=list)
    aliases: dict[str, str] = field(default_factory=dict)
    global_aliases: dict[str, str] = field(default_factory=dict)
    structs: dict[str, str] = field(default_factory=dict)
    struct_caps: dict[str, set[str]] = field(default_factory=dict)
    impl_types: dict[str, str] = field(default_factory=dict)
    fns: dict[str, FnInfo] = field(default_factory=dict)
    exports: dict[str, str] = field(default_factory=dict)
    glob_imports: list[str] = field(default_factory=list)


def rel(path: str, root: str) -> str:
    return os.path.relpath(path, root).replace(os.sep, "/")


def canonicalize_path(module: str, path: str) -> str:
    if path.startswith("super::"):
        parent = "::".join(module.split("::")[:-1])
        rest = path[len("super::") :]
        return join_path(parent, rest) if rest else parent
    if path.startswith("self::"):
        rest = path[len("self::") :]
        return join_path(module, rest) if rest else module
    return path


def tests_rs_cfg_gated(root: str, abs_path: str) -> bool:
    """True when the sibling mod.rs includes `#[cfg(test)] mod tests;`."""
    parent = os.path.dirname(abs_path)
    for candidate in ("mod.rs", "lib.rs"):
        mod_path = os.path.join(parent, candidate)
        if not os.path.isfile(mod_path):
            continue
        try:
            raw = open(mod_path, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        if re.search(r"#\[cfg\s*\(\s*test\s*\)\]\s*(?:pub\s+)?mod\s+tests\s*;", raw):
            return True
    return False


def is_domain_path(path: str) -> bool:
    if path in ORCHESTRATION_FILES:
        return False
    if path == "src/network/core_inventory.rs" or path.startswith(
        "src/network/core_inventory/"
    ):
        return True
    return any(
        path == p.rstrip("/") or path.startswith(p)
        for p in DOMAIN_PREFIXES
        if p.endswith("/")
    ) or path.startswith("src/logic/")


def layer_of(path: str) -> str:
    if path in ORCHESTRATION_FILES:
        return "ORCHESTRATION"
    if path.startswith("src/console/") or path == "src/console.rs":
        return "CONSOLE"
    if path.startswith("src/tui/") or path == "src/tui.rs":
        return "TUI"
    if path.startswith("src/network/listener/"):
        return "LISTENER"
    if path.startswith("src/network/session/"):
        return "SESSION"
    if path == "src/network/outbound.rs":
        return "OUTBOUND_ADAPTER"
    if path == "src/network/runtime.rs":
        return "RUNTIME"
    if path.startswith("src/network/wire/"):
        return "WIRE"
    if is_domain_path(path):
        return "DOMAIN"
    return "OTHER"


def walk_rust_files(root: str) -> list[str]:
    files: list[str] = []
    src = os.path.join(root, "src")
    if not os.path.isdir(src):
        return files
    for dp, _, fns in os.walk(src):
        for fn in fns:
            if fn.endswith(".rs"):
                files.append(os.path.join(dp, fn))
    files.sort()
    return files


def load_exceptions(root: str) -> dict[str, set[tuple[str, str]]]:
    """path -> {(code, symbol)}."""
    path = os.path.join(root, "migration-reports/architecture-exceptions.tsv")
    mapping: dict[str, set[tuple[str, str]]] = defaultdict(set)
    if not os.path.isfile(path):
        return mapping
    with open(path, encoding="utf-8") as fh:
        header = fh.readline().rstrip("\n")
        cols = header.split("\t")
        if cols[:6] != ["path", "code", "symbol", "rationale", "reviewer", "revisit"]:
            print(
                "EXCEPTIONS_MALFORMED\t.\theader\tmigration-reports/architecture-exceptions.tsv:1",
                file=sys.stdout,
            )
            sys.exit(2)
        for i, line in enumerate(fh, start=2):
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) != 6:
                print(f"EXCEPTIONS_MALFORMED\t.\tcolumns\t{path}:{i}")
                sys.exit(2)
            file_path, code, symbol, rationale, reviewer, revisit = parts
            if file_path not in ALLOWED_EXCEPTION_PATHS:
                print(f"EXCEPTIONS_UNKNOWN_PATH\t.\t{file_path}\t{path}:{i}")
                sys.exit(2)
            if code not in ALLOWED_EXCEPTION_CODES:
                print(f"EXCEPTIONS_BAD_CODE\t.\t{code}\t{path}:{i}")
                sys.exit(2)
            if len(rationale.strip()) < 40:
                print(f"EXCEPTIONS_WEAK_JUSTIFICATION\t.\t{file_path}\t{path}:{i}")
                sys.exit(2)
            if not reviewer.strip() or not symbol.strip():
                print(f"EXCEPTIONS_INCOMPLETE\t.\t{file_path}\t{path}:{i}")
                sys.exit(2)
            mapping[file_path].add((code, symbol.strip()))
    return mapping


def type_has_connection(type_src: str, aliases: dict[str, str]) -> bool:
    t = re.sub(r"\s+", " ", type_src)
    for _ in range(6):
        changed = False
        for name, body in aliases.items():
            if not name:
                continue
            if "::" in name:
                if name in t:
                    t = t.replace(name, f" {body} ")
                    changed = True
            elif re.search(rf"\b{re.escape(name)}\b", t):
                t = re.sub(rf"\b{re.escape(name)}\b", f" {body} ", t)
                changed = True
        if not changed:
            break
    return bool(RE_REGISTRY.search(t) or RE_PENDING.search(t))


def type_has_socket(type_src: str) -> bool:
    return bool(RE_SOCKET.search(type_src))


def classify_path(path: str) -> tuple[str | None, str | None]:
    """Return (code, capability) for a crate path, or (None, None)."""
    p = path.rstrip(":")
    if p == "crate::network::listener" or p.startswith("crate::network::listener::"):
        return "ARCH001", CAP_LISTENER
    if p == "crate::network::runtime" or p.startswith("crate::network::runtime::"):
        return "ARCH002", CAP_RUNTIME
    if p == "crate::console" or p.startswith("crate::console::"):
        return "ARCH002", CAP_RUNTIME
    if p == "crate::tui" or p.startswith("crate::tui::"):
        return "ARCH002", CAP_RUNTIME
    if p in DELIVERY_FNS:
        return "ARCH004", CAP_OUTBOUND
    last = p.split("::")[-1]
    if last in DELIVERY_NAMES and (
        "::outbound::" in p
        or "::listener::" in p
        or "::session::" in p
        or p.startswith("crate::network::outbound::")
        or p.startswith("crate::network::listener::")
        or p.startswith("crate::network::session::")
    ):
        return "ARCH004", CAP_OUTBOUND
    if last == "broadcast" and (
        p.startswith("crate::network::outbound::")
        or p.startswith("crate::network::listener::")
    ):
        return "ARCH004", CAP_OUTBOUND
    if p == "crate::network::session" or p.startswith("crate::network::session::"):
        return "ARCH004", CAP_OUTBOUND
    return None, None


def is_allowed_outbound_item(path: str) -> bool:
    last = path.split("::")[-1]
    if last in {"FrameEmit", "NoopEmit", "NOOP"}:
        return True
    if "::FrameEmit::" in path or path.endswith("::FrameEmit"):
        return True
    if "::NoopEmit::" in path:
        return True
    return False


def parse_file(root: str, abs_path: str) -> FileInfo | None:
    rpath = rel(abs_path, root)
    if os.path.basename(abs_path) == "tests.rs" and tests_rs_cfg_gated(root, abs_path):
        return None
    try:
        raw = open(abs_path, encoding="utf-8", errors="replace").read()
    except OSError:
        return None
    src = mask_cfg_test_modules(strip_block_comments_and_strings(raw))
    info = FileInfo(
        path=rpath,
        module=module_path_for(rpath),
        layer=layer_of(rpath),
        src=src,
    )
    for line, tree, is_reexport in iter_use_statements(src):
        for local, path, is_glob in parse_use_tree("", tree):
            path = canonicalize_path(info.module, path)
            imp = Import(local, path, is_glob, is_reexport, line)
            info.imports.append(imp)
            if is_glob:
                info.glob_imports.append(path)
            elif is_reexport:
                info.exports[local] = path
            else:
                info.exports.setdefault(local, path)
    for line, name, body in extract_type_aliases(src):
        info.aliases[name] = body
        info.exports.setdefault(name, join_path(info.module, name))
        info.global_aliases[join_path(info.module, name)] = body
    for line, name, body in extract_structs(src):
        info.structs[name] = body
        caps: set[str] = set()
        if type_has_connection(body, info.aliases):
            caps.add(CAP_CONNECTION)
        info.struct_caps[name] = caps
        info.exports.setdefault(name, join_path(info.module, name))
    for impl_name, impl_body in extract_impl_types(src):
        info.impl_types[impl_name] = impl_body
    for line, name, params, body in extract_functions(src):
        fn = FnInfo(
            name=name,
            line=line,
            params=params,
            body=body,
            module=info.module,
            file=rpath,
        )
        info.fns[name] = fn
        info.exports.setdefault(name, join_path(info.module, name))
    return info


def resolve_name(info: FileInfo, name: str, index: dict[str, FileInfo]) -> str | None:
    if name in info.aliases:
        # type alias local
        pass
    if name in info.exports and info.exports[name] != join_path(info.module, name):
        return info.exports[name]
    for imp in info.imports:
        if not imp.is_glob and imp.local == name:
            return imp.path
    if name in info.fns:
        return join_path(info.module, name)
    # follow same-module
    key = join_path(info.module, name)
    if key in _export_index(index):
        return key
    return None


_EXPORT_CACHE: dict[str, str] | None = None
_INDEX_ID: int | None = None


def _export_index(index: dict[str, FileInfo]) -> dict[str, str]:
    global _EXPORT_CACHE, _INDEX_ID
    ident = id(index)
    if _EXPORT_CACHE is not None and _INDEX_ID == ident:
        return _EXPORT_CACHE
    exports: dict[str, str] = {}
    for info in index.values():
        for local, path in info.exports.items():
            exports[join_path(info.module, local)] = path
            exports[path] = path
        for fn in info.fns:
            exports[join_path(info.module, fn)] = join_path(info.module, fn)
    _EXPORT_CACHE = exports
    _INDEX_ID = ident
    return exports


def follow_exports(path: str, index: dict[str, FileInfo], depth: int = 0) -> str:
    if depth > 6:
        return path
    exports = _export_index(index)
    target = exports.get(path)
    if target and target != path:
        return follow_exports(target, index, depth + 1)
    # module::name where module re-exports name
    if "::" in path:
        mod, name = path.rsplit("::", 1)
        # find file with that module
        for info in index.values():
            if info.module == mod and name in info.exports:
                nxt = info.exports[name]
                if nxt != path:
                    return follow_exports(nxt, index, depth + 1)
    return path


def lookup_fn(path: str, index: dict[str, FileInfo]) -> FnInfo | None:
    resolved = follow_exports(path, index)
    for info in index.values():
        full = join_path(info.module, resolved.split("::")[-1])
        if join_path(info.module, resolved.split("::")[-1]) == resolved or resolved.startswith(
            info.module + "::"
        ):
            name = resolved[len(info.module) + 2 :] if resolved.startswith(info.module + "::") else resolved.split("::")[-1]
            if info.module == resolved.rsplit("::", 1)[0] if "::" in resolved else "":
                fn = info.fns.get(name)
                if fn:
                    return fn
    if "::" in resolved:
        mod, name = resolved.rsplit("::", 1)
        for info in index.values():
            if info.module == mod and name in info.fns:
                return info.fns[name]
    return None


CALL_RE = re.compile(
    r"(?<![\w.])((?:(?:crate|super|self)::)?[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)\s*\("
)
METHOD_RE = re.compile(
    r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\("
)


def collect_callees(info: FileInfo, fn: FnInfo, index: dict[str, FileInfo]) -> None:
    body = fn.body
    frame_emit_in_scope = any(
        is_allowed_outbound_item(imp.path) or "FrameEmit" in imp.path
        for imp in info.imports
    ) or bool(RE_FRAME_EMIT.search(fn.params) or RE_FRAME_EMIT.search(info.src))
    frame_emit_methods = {"broadcast", "enqueue_to", "for_each_connection", "connection_ip"}
    for m in METHOD_RE.finditer(body):
        method = m.group(1)
        line = fn.line + body[: m.start()].count("\n")
        if method in frame_emit_methods and frame_emit_in_scope:
            continue
        for other in index.values():
            if other.layer == "DOMAIN":
                continue
            target = other.fns.get(method)
            if target is None:
                continue
            blob = target.params + " " + target.body
            if (
                target.caps & {CAP_LISTENER, CAP_OUTBOUND, CAP_CONNECTION}
                or any(p in blob for p in DELIVERY_FNS)
                or "crate::network::listener" in blob
                or method in DELIVERY_NAMES
            ):
                fn.callees.append((join_path(other.module, method), line))
    for m in CALL_RE.finditer(body):
        raw = m.group(1)
        prev = body[m.start() - 1] if m.start() > 0 else ""
        if prev == ".":
            continue
        line = fn.line + body[: m.start()].count("\n")
        resolved = resolve_call(info, raw, index)
        fn.callees.append((resolved or raw, line))


def resolve_call(info: FileInfo, raw: str, index: dict[str, FileInfo]) -> str | None:
    raw = canonicalize_path(info.module, raw)
    if raw.startswith("crate::"):
        return follow_exports(raw, index)
    if raw.startswith("super::"):
        parent = "::".join(info.module.split("::")[:-1])
        return follow_exports(join_path(parent, raw[len("super::") :]), index)
    if raw.startswith("self::"):
        return follow_exports(join_path(info.module, raw[len("self::") :]), index)
    if "::" in raw:
        head, rest = raw.split("::", 1)
        for imp in info.imports:
            if not imp.is_glob and imp.local == head:
                return follow_exports(join_path(imp.path, rest), index)
        # crate-relative from current module
        return follow_exports(join_path(info.module, raw), index)
    resolved = resolve_name(info, raw, index)
    if resolved:
        return follow_exports(resolved, index)
    for glob in info.glob_imports:
        candidate = follow_exports(join_path(glob, raw), index)
        if lookup_fn(candidate, index) or classify_path(candidate)[0]:
            return candidate
    return None


def seed_fn_caps(fn: FnInfo, info: FileInfo, aliases: dict[str, str]) -> None:
    blob = fn.params + " " + fn.body
    merged_aliases = dict(aliases)
    merged_aliases.update(info.aliases)
    if type_has_connection(fn.params, merged_aliases):
        fn.caps.add(CAP_CONNECTION)
    if type_has_socket(fn.params):
        fn.caps.add(CAP_OUTBOUND)
    full = join_path(fn.module, fn.name)
    code, cap = classify_path(full)
    if cap:
        fn.caps.add(cap)
    if full in DELIVERY_FNS or join_path("crate::network::outbound", fn.name) in DELIVERY_FNS:
        fn.caps.add(CAP_OUTBOUND)
    # listener paths in body
    if "crate::network::listener" in blob or re.search(
        r"\blistener::", blob
    ):
        # only if listener is imported or qualified
        if any(
            imp.path.startswith("crate::network::listener") for imp in info.imports
        ) or "crate::network::listener" in blob:
            fn.caps.add(CAP_LISTENER)
            fn.delivery_mentions.append("crate::network::listener")
    for raw_path in DELIVERY_FNS:
        short = raw_path.split("::")[-1]
        if short == "broadcast":
            continue
        if short + "(" in fn.body or raw_path in fn.body:
            fn.caps.add(CAP_OUTBOUND)
            fn.delivery_mentions.append(raw_path)
    # free-function broadcast in adapter modules
    if fn.name in {"broadcast", "broadcast_except"} and info.layer in {
        "OUTBOUND_ADAPTER",
        "LISTENER",
        "WIRE",
        "SESSION",
    }:
        if type_has_connection(fn.params, merged_aliases) or CAP_CONNECTION in fn.caps:
            fn.caps.add(CAP_OUTBOUND)


def apply_one_hop(index: dict[str, FileInfo]) -> None:
    fn_by_path: dict[str, FnInfo] = {}
    for info in index.values():
        for fn in info.fns.values():
            fn_by_path[join_path(fn.module, fn.name)] = fn
    for _ in range(4):
        changed = False
        for info in index.values():
            for fn in info.fns.values():
                before = set(fn.caps)
                for callee, _line in fn.callees:
                    resolved = follow_exports(canonicalize_path(info.module, callee), index)
                    target = fn_by_path.get(resolved) or lookup_fn(resolved, index)
                    if target is None:
                        code, cap = classify_path(resolved)
                        if cap:
                            fn.caps.add(cap)
                            fn.delivery_mentions.append(resolved)
                        continue
                    fn.caps |= set(target.caps)
                    if target.caps:
                        fn.delivery_mentions.append(join_path(target.module, target.name))
                if fn.caps != before:
                    changed = True
        if not changed:
            break


def _merged_aliases(info: FileInfo, index: dict[str, FileInfo]) -> dict[str, str]:
    global_map: dict[str, str] = {}
    for other in index.values():
        global_map.update(other.global_aliases)
    merged = dict(info.aliases)
    merged.update(global_map)
    for imp in info.imports:
        if imp.is_glob:
            prefix = imp.path
            for fqn, body in global_map.items():
                if fqn == prefix or fqn.startswith(prefix + "::"):
                    merged[fqn.split("::")[-1]] = body
            continue
        merged[imp.local] = global_map.get(imp.path, imp.path)
    return merged


def domain_fn_exposes_connection(info: FileInfo, fn: FnInfo, index: dict[str, FileInfo]) -> bool:
    params = fn.params
    aliases = _merged_aliases(info, index)
    if type_has_connection(params, aliases):
        return True
    if type_has_socket(params):
        return True
    self_param = re.search(r"(?:^|,)\s*(?:&(?:mut\s+)?|mut\s+)?self\b", params)
    if self_param:
        for sname, caps in info.struct_caps.items():
            if CAP_CONNECTION in caps and sname in info.impl_types:
                return True
    # struct wrappers in this file
    for sname, caps in info.struct_caps.items():
        if CAP_CONNECTION in caps and re.search(rf"\b{re.escape(sname)}\b", params):
            return True
    # imported types that wrap the registry
    for imp in info.imports:
        if imp.is_glob:
            continue
        if re.search(rf"\b{re.escape(imp.local)}\b", params):
            target = follow_exports(imp.path, index)
            for other in index.values():
                if other.module == target or join_path(other.module, imp.local) == target:
                    if CAP_CONNECTION in other.struct_caps.get(imp.local, set()):
                        return True
                name = target.split("::")[-1]
                if name in other.struct_caps and CAP_CONNECTION in other.struct_caps[name]:
                    if other.module == target.rsplit("::", 1)[0]:
                        return True
            if type_has_connection(info.aliases.get(imp.local, ""), aliases):
                return True
            if type_has_connection(imp.path, aliases):
                return True
    for name, body in aliases.items():
        short = name.split("::")[-1]
        if "::" in name:
            continue
        if type_has_connection(body, aliases) and re.search(
            rf"\b{re.escape(short)}\b", params
        ):
            return True
    return False


def pretty_mod(path: str) -> str:
    p = path
    if p.startswith("src/"):
        p = p[4:]
    if p.endswith(".rs"):
        p = p[:-3]
    if p.endswith("/mod"):
        p = p[: -len("/mod")]
    return p.replace("/", "::")


def excepted(
    exceptions: dict[str, set[tuple[str, str]]], path: str, code: str, symbol: str
) -> bool:
    allowed = exceptions.get(path, set())
    if (code, symbol) in allowed:
        return True
    return False


def emit_diag(
    diags: list[Diagnostic],
    exceptions: dict[str, set[tuple[str, str]]],
    code: str,
    source: str,
    line: int,
    target: str,
    capability: str,
    chain: str,
) -> None:
    if excepted(exceptions, source, code, target):
        return
    diags.append(
        Diagnostic(
            code=code,
            source=source,
            line=line,
            target=target,
            capability=capability,
            chain=chain,
            evidence=f"{source}:{line}",
            hint=HINTS.get(code, ""),
        )
    )


def scan_file_rules(
    info: FileInfo,
    index: dict[str, FileInfo],
    exceptions: dict[str, set[tuple[str, str]]],
    diags: list[Diagnostic],
) -> None:
    src_mod = pretty_mod(info.path)

    if info.layer == "WIRE":
        for imp in info.imports:
            path = follow_exports(imp.path, index)
            if path == "crate::network::listener" or path.startswith(
                "crate::network::listener::"
            ):
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH005",
                    info.path,
                    imp.line,
                    path,
                    CAP_LISTENER,
                    f"{src_mod} -> {path}",
                )
        for m in re.finditer(r"crate::network::listener(?:::[A-Za-z0-9_]+)*", info.src):
            emit_diag(
                diags,
                exceptions,
                "ARCH005",
                info.path,
                info.src[: m.start()].count("\n") + 1,
                m.group(0),
                CAP_LISTENER,
                f"{src_mod} -> {m.group(0)}",
            )
        for fn in info.fns.values():
            for callee, line in fn.callees:
                resolved = follow_exports(callee, index)
                if resolved.startswith("crate::network::listener"):
                    emit_diag(
                        diags,
                        exceptions,
                        "ARCH005",
                        info.path,
                        line,
                        resolved,
                        CAP_LISTENER,
                        f"{src_mod} -> {join_path(fn.module, fn.name)} -> {resolved}",
                    )

    if info.layer != "DOMAIN":
        return

    # Direct imports
    for imp in info.imports:
        resolved = follow_exports(imp.path, index)
        origin = resolved
        code, cap = classify_path(imp.path)
        code_r, cap_r = classify_path(resolved)
        use_code = code_r or code
        use_cap = cap_r or cap
        if imp.is_glob:
            gcode, gcap = classify_path(imp.path if imp.path.endswith("::") is False else imp.path)
            # glob of listener/outbound/runtime/session/console
            if imp.path.startswith("crate::network::listener"):
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH001",
                    info.path,
                    imp.line,
                    imp.path + "::*",
                    CAP_LISTENER,
                    f"{src_mod} -> {imp.path}::*",
                )
                continue
            if imp.path.startswith("crate::network::outbound") and not is_allowed_outbound_item(
                imp.path
            ):
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH006",
                    info.path,
                    imp.line,
                    imp.path + "::*",
                    CAP_OUTBOUND,
                    f"{src_mod} -> {imp.path}::*",
                )
                continue
            if (
                imp.path.startswith("crate::network::runtime")
                or imp.path.startswith("crate::console")
                or imp.path.startswith("crate::tui")
            ):
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH002",
                    info.path,
                    imp.line,
                    imp.path + "::*",
                    CAP_RUNTIME,
                    f"{src_mod} -> {imp.path}::*",
                )
                continue
            if imp.path.startswith("crate::network::session"):
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH004",
                    info.path,
                    imp.line,
                    imp.path + "::*",
                    CAP_OUTBOUND,
                    f"{src_mod} -> {imp.path}::*",
                )
                continue
        if is_allowed_outbound_item(imp.path) or is_allowed_outbound_item(resolved):
            continue
        if use_code == "ARCH001":
            emit_diag(
                diags,
                exceptions,
                "ARCH001",
                info.path,
                imp.line,
                origin,
                CAP_LISTENER,
                f"{src_mod} -> {origin}",
            )
            continue
        if use_code == "ARCH002":
            emit_diag(
                diags,
                exceptions,
                "ARCH002",
                info.path,
                imp.line,
                origin,
                CAP_RUNTIME,
                f"{src_mod} -> {origin}",
            )
            continue
        aliased = imp.local != origin.split("::")[-1] or (
            origin != imp.path and classify_path(origin)[0] in {"ARCH001", "ARCH004", "ARCH002"}
        )
        if use_code == "ARCH004":
            code_out = "ARCH006" if (aliased or origin != imp.path) else "ARCH004"
            emit_diag(
                diags,
                exceptions,
                code_out,
                info.path,
                imp.line,
                origin,
                CAP_OUTBOUND,
                f"{src_mod} -> {imp.path} -> {origin}"
                if origin != imp.path
                else f"{src_mod} -> {origin}",
            )
            continue
        # imported helper whose definition carries a forbidden capability
        fn = lookup_fn(resolved, index)
        if fn and (fn.caps & {CAP_LISTENER, CAP_OUTBOUND, CAP_CONNECTION}):
            if CAP_LISTENER in fn.caps:
                cap = CAP_LISTENER
                code_hit = "ARCH004"
            elif CAP_OUTBOUND in fn.caps:
                cap = CAP_OUTBOUND
                code_hit = "ARCH006" if origin != imp.path else "ARCH004"
            else:
                cap = CAP_CONNECTION
                code_hit = "ARCH003"
            chain_tail = fn.delivery_mentions[0] if fn.delivery_mentions else join_path(fn.module, fn.name)
            emit_diag(
                diags,
                exceptions,
                code_hit,
                info.path,
                imp.line,
                join_path(fn.module, fn.name),
                cap,
                f"{src_mod} -> {imp.path} -> {chain_tail}",
            )

    # Qualified paths in source (not only use statements)
    for m in re.finditer(r"crate::network::listener(?:::[A-Za-z0-9_]+)*", info.src):
        emit_diag(
            diags,
            exceptions,
            "ARCH001",
            info.path,
            info.src[: m.start()].count("\n") + 1,
            m.group(0),
            CAP_LISTENER,
            f"{src_mod} -> {m.group(0)}",
        )
    for m in re.finditer(r"crate::network::runtime(?:::[A-Za-z0-9_]+)*", info.src):
        emit_diag(
            diags,
            exceptions,
            "ARCH002",
            info.path,
            info.src[: m.start()].count("\n") + 1,
            m.group(0),
            CAP_RUNTIME,
            f"{src_mod} -> {m.group(0)}",
        )
    for m in re.finditer(r"crate::console(?:::[A-Za-z0-9_]+)*", info.src):
        emit_diag(
            diags,
            exceptions,
            "ARCH002",
            info.path,
            info.src[: m.start()].count("\n") + 1,
            m.group(0),
            CAP_RUNTIME,
            f"{src_mod} -> {m.group(0)}",
        )
    for dfn in sorted(DELIVERY_FNS):
        start = 0
        while True:
            idx = info.src.find(dfn, start)
            if idx < 0:
                break
            emit_diag(
                diags,
                exceptions,
                "ARCH004",
                info.path,
                info.src[:idx].count("\n") + 1,
                dfn,
                CAP_OUTBOUND,
                f"{src_mod} -> {dfn}",
            )
            start = idx + len(dfn)
    for m in re.finditer(r"crate::tui(?:::[A-Za-z0-9_]+)*", info.src):
        emit_diag(
            diags,
            exceptions,
            "ARCH002",
            info.path,
            info.src[: m.start()].count("\n") + 1,
            m.group(0),
            CAP_RUNTIME,
            f"{src_mod} -> {m.group(0)}",
        )

    for fn in info.fns.values():
        if domain_fn_exposes_connection(info, fn, index):
            emit_diag(
                diags,
                exceptions,
                "ARCH003",
                info.path,
                fn.line,
                "PendingConnection",
                CAP_CONNECTION,
                f"{src_mod}::{fn.name} exposes NETWORK_CONNECTION",
            )
        for callee, line in fn.callees:
            resolved = follow_exports(callee, index) if callee.startswith("crate::") or "::" in callee else (
                follow_exports(callee, index) if lookup_fn(follow_exports(callee, index), index) else callee
            )
            if not callee.startswith("crate::") and "::" not in callee:
                rec = resolve_call(info, callee, index)
                resolved = rec or callee
            else:
                resolved = follow_exports(callee, index)

            code, cap = classify_path(resolved)
            target_fn = lookup_fn(resolved, index)

            if is_allowed_outbound_item(resolved):
                continue

            if code == "ARCH001" or (target_fn and CAP_LISTENER in target_fn.caps):
                chain = f"{src_mod} -> {resolved}"
                if target_fn and target_fn.delivery_mentions:
                    chain = f"{src_mod} -> {resolved} -> {target_fn.delivery_mentions[0]}"
                    emit_diag(
                        diags,
                        exceptions,
                        "ARCH004",
                        info.path,
                        line,
                        resolved,
                        CAP_LISTENER,
                        chain,
                    )
                elif code == "ARCH001":
                    emit_diag(
                        diags,
                        exceptions,
                        "ARCH001",
                        info.path,
                        line,
                        resolved,
                        CAP_LISTENER,
                        chain,
                    )
                else:
                    emit_diag(
                        diags,
                        exceptions,
                        "ARCH004",
                        info.path,
                        line,
                        resolved,
                        CAP_LISTENER,
                        chain,
                    )
                continue
            if code == "ARCH002":
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH002",
                    info.path,
                    line,
                    resolved,
                    CAP_RUNTIME,
                    f"{src_mod} -> {resolved}",
                )
                continue
            if code == "ARCH004" or (target_fn and CAP_OUTBOUND in target_fn.caps):
                chain = f"{src_mod} -> {resolved}"
                if target_fn and target_fn.delivery_mentions:
                    chain += f" -> {target_fn.delivery_mentions[0]}"
                aliased = resolved != callee and callee.split("::")[-1] != resolved.split("::")[-1]
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH006" if aliased else "ARCH004",
                    info.path,
                    line,
                    resolved,
                    CAP_OUTBOUND,
                    chain,
                )
                continue
            if target_fn and CAP_CONNECTION in target_fn.caps and info.layer == "DOMAIN":
                emit_diag(
                    diags,
                    exceptions,
                    "ARCH004",
                    info.path,
                    line,
                    resolved,
                    CAP_CONNECTION,
                    f"{src_mod} -> {resolved}",
                )
                continue

            # Unresolved suspicious delivery names
            simple = callee.split("::")[-1]
            if simple in DELIVERY_NAMES or (
                simple == "broadcast"
                and any(
                    imp.path.startswith("crate::network::outbound")
                    or imp.path.startswith("crate::network::listener")
                    or imp.path.startswith("crate::network::wire")
                    for imp in info.imports
                )
                and not any(
                    is_allowed_outbound_item(imp.path) and imp.local in {"FrameEmit", "NoopEmit"}
                    for imp in info.imports
                )
            ):
                if target_fn is None and code is None and not callee.startswith("self"):
                    # FrameEmit method already skipped; unresolved free call
                    if simple == "broadcast":
                        # only if imported from a delivery module
                        delivery_import = any(
                            (not imp.is_glob and imp.local == "broadcast")
                            or (
                                imp.is_glob
                                and (
                                    imp.path.startswith("crate::network::outbound")
                                    or imp.path.startswith("crate::network::listener")
                                )
                            )
                            for imp in info.imports
                        )
                        if not delivery_import:
                            continue
                    emit_diag(
                        diags,
                        exceptions,
                        "ARCH900",
                        info.path,
                        line,
                        callee,
                        CAP_OUTBOUND,
                        f"{src_mod} -> {callee} (unresolved)",
                    )


def scan_root(root: str) -> tuple[list[Diagnostic], list[str]]:
    global _EXPORT_CACHE, _INDEX_ID
    _EXPORT_CACHE = None
    _INDEX_ID = None
    exceptions = load_exceptions(root)
    index: dict[str, FileInfo] = {}
    for abs_path in walk_rust_files(root):
        rpath = rel(abs_path, root)
        if os.path.basename(abs_path) == "tests.rs" and tests_rs_cfg_gated(root, abs_path):
            continue
        info = parse_file(root, abs_path)
        if info is None:
            continue
        index[rpath] = info
    _EXPORT_CACHE = None
    for info in index.values():
        for fn in info.fns.values():
            seed_fn_caps(fn, info, info.aliases)
    for info in index.values():
        for fn in info.fns.values():
            collect_callees(info, fn, index)
    apply_one_hop(index)
    diags: list[Diagnostic] = []
    for info in index.values():
        scan_file_rules(info, index, exceptions, diags)
    # Dedup
    seen: set[tuple] = set()
    uniq: list[Diagnostic] = []
    for d in diags:
        key = (d.code, d.source, d.line, d.target, d.capability)
        if key in seen:
            continue
        seen.add(key)
        uniq.append(d)
    uniq.sort(key=lambda d: (d.source, d.line, d.code, d.target))
    return uniq, list(RULE_CLASSES)


def write_ledger_atomic(path: str, diags: list[Diagnostic], classes: list[str]) -> None:
    directory = os.path.dirname(path)
    os.makedirs(directory, exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write("\t".join(LEDGER_COLUMNS) + "\n")
        fh.write(f"# RULE_CLASSES_COMPLETED: {','.join(classes)}\n")
        for d in diags:
            fh.write(d.tsv() + "\n")
        fh.flush()
        os.fsync(fh.fileno())
    # Validate temp before replace
    with open(tmp, encoding="utf-8") as fh:
        header = fh.readline().rstrip("\n")
        if header.split("\t") != list(LEDGER_COLUMNS):
            os.remove(tmp)
            raise SystemExit("ledger temp header invalid; canonical ledger left unchanged")
        found_classes = False
        for line in fh:
            if line.startswith("# RULE_CLASSES_COMPLETED:"):
                found_classes = True
                got = line.split(":", 1)[1].strip().split(",")
                if got != classes:
                    os.remove(tmp)
                    raise SystemExit("ledger temp missing rule-class coverage")
                break
        if not found_classes:
            os.remove(tmp)
            raise SystemExit("ledger temp missing RULE_CLASSES_COMPLETED")
    os.replace(tmp, path)


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    root = "."
    emit_ledger = False
    ledger_path = None
    pretty = False
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg in ("-h", "--help"):
            print(
                "usage: scan.py [ROOT] [--ledger PATH] [--pretty]\n"
                "  stdout: TSV diagnostics (no header)\n"
                "  stderr: ARCHGUARD_SCAN_COMPLETE\\tCODES",
                file=sys.stderr,
            )
            return 0
        if arg == "--ledger":
            emit_ledger = True
            i += 1
            ledger_path = argv[i] if i < len(argv) else "migration-reports/architecture-violations.tsv"
        elif arg == "--pretty":
            pretty = True
        elif not arg.startswith("-"):
            root = arg
        i += 1
    root = os.path.abspath(root)
    try:
        diags, classes = scan_root(root)
    except SystemExit:
        raise
    except Exception as exc:
        print(f"ARCHGUARD_TOOL_FAIL\t{type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    for d in diags:
        if pretty:
            print(d.code, file=sys.stderr)
            print(f"{d.source}:{d.line}", file=sys.stderr)
            print(d.chain, file=sys.stderr)
            print(f"forbidden capability: {d.capability}", file=sys.stderr)
            print(f"remediation: {d.hint}", file=sys.stderr)
            print(file=sys.stderr)
        print(d.tsv())
    print("ARCHGUARD_SCAN_COMPLETE\t" + ",".join(classes), file=sys.stderr)
    if emit_ledger:
        path = ledger_path or os.path.join(root, "migration-reports/architecture-violations.tsv")
        if not os.path.isabs(path):
            path = os.path.join(root, path)
        try:
            write_ledger_atomic(path, diags, classes)
        except SystemExit as exc:
            print(str(exc), file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
