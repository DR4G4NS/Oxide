"""Bounded Rust surface parser: comments, cfg(test), use trees, items."""
from __future__ import annotations

import re
from typing import Iterator

VIS_RE = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?")


def strip_block_comments_and_strings(src: str) -> str:
    """Replace comments and string/char literals with spaces (keep newlines)."""
    out: list[str] = []
    i = 0
    n = len(src)
    while i < n:
        ch = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if ch == "/" and nxt == "/":
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
            continue
        if ch == "/" and nxt == "*":
            out.append(" ")
            out.append(" ")
            i += 2
            while i < n and not (src[i] == "*" and i + 1 < n and src[i + 1] == "/"):
                out.append("\n" if src[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append(" ")
                out.append(" ")
                i += 2
            continue
        if ch == "b" and nxt in ('"', "'"):
            out.append(" ")
            i += 1
            continue
        if ch == "'":
            j = i + 1
            if j < n and (src[j].isalpha() or src[j] == "_"):
                k = j + 1
                while k < n and (src[k].isalnum() or src[k] == "_"):
                    k += 1
                if k >= n or src[k] != "'":
                    # Lifetime `'a` / `'static`, not a char literal.
                    out.append(ch)
                    i += 1
                    continue
        if ch in ('"', "'"):
            quote = ch
            raw = False
            out.append(" ")
            i += 1
            while i < n:
                if src[i] == "\\" and not raw:
                    out.append(" ")
                    if i + 1 < n:
                        out.append("\n" if src[i + 1] == "\n" else " ")
                        i += 2
                        continue
                if src[i] == quote:
                    out.append(" ")
                    i += 1
                    break
                out.append("\n" if src[i] == "\n" else " ")
                i += 1
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def mask_cfg_test_modules(src: str) -> str:
    """Blank out #[cfg(test)] mod ... { } regions and `mod tests;` lines."""
    lines = src.splitlines(keepends=True)
    i = 0
    n = len(lines)
    out: list[str] = []
    while i < n:
        stripped = lines[i].lstrip()
        if stripped.startswith("#[cfg(test)]"):
            header = lines[i]
            j = i
            buf = header
            if "mod " not in header and i + 1 < n:
                j = i + 1
                buf += lines[j]
            if re.search(r"mod\s+\w+\s*;", buf):
                for k in range(i, j + 1):
                    out.append("\n" * lines[k].count("\n") or "\n")
                i = j + 1
                continue
            if "{" in buf:
                start_line = i
                rest = "".join(lines[start_line:])
                brace_at = rest.find("{")
                if brace_at < 0:
                    out.append(lines[i])
                    i += 1
                    continue
                k = _match_brace(rest, brace_at)
                blanked = "".join("\n" if ch == "\n" else " " for ch in rest[:k])
                out.append(blanked)
                leftover = rest[k:]
                lines = leftover.splitlines(keepends=True)
                i = 0
                n = len(lines)
                continue
        out.append(lines[i])
        i += 1
    return "".join(out)


def _match_brace(src: str, start: int) -> int:
    depth = 0
    i = start
    n = len(src)
    while i < n:
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


def match_pairs(src: str, start: int, open_ch: str, close_ch: str) -> int:
    if start >= len(src) or src[start] != open_ch:
        return -1
    depth = 0
    i = start
    n = len(src)
    while i < n:
        ch = src[i]
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def split_top_level(s: str, sep: str = ",") -> list[str]:
    parts: list[str] = []
    buf: list[str] = []
    depth_brace = depth_paren = depth_angle = 0
    i = 0
    n = len(s)
    while i < n:
        ch = s[i]
        if ch == "{":
            depth_brace += 1
        elif ch == "}":
            depth_brace -= 1
        elif ch == "(":
            depth_paren += 1
        elif ch == ")":
            depth_paren -= 1
        elif ch == "<":
            depth_angle += 1
        elif ch == ">":
            depth_angle -= 1
        if ch == sep and depth_brace == depth_paren == 0 and depth_angle <= 0:
            parts.append("".join(buf).strip())
            buf = []
            i += 1
            continue
        buf.append(ch)
        i += 1
    tail = "".join(buf).strip()
    if tail:
        parts.append(tail)
    return parts


def join_path(*parts: str) -> str:
    segs: list[str] = []
    for part in parts:
        for seg in part.split("::"):
            if seg:
                segs.append(seg)
    return "::".join(segs)


def parse_use_tree(prefix: str, tree: str) -> list[tuple[str, str, bool]]:
    """Return (local_name, resolved_path, is_glob) for a use tree fragment."""
    tree = tree.strip().rstrip(",").strip()
    if not tree:
        return []
    if tree.startswith("{"):
        end = match_pairs(tree, 0, "{", "}")
        inner = tree[1:end] if end >= 0 else tree[1:]
        out: list[tuple[str, str, bool]] = []
        for item in split_top_level(inner):
            out.extend(parse_use_tree(prefix, item))
        return out
    if tree.endswith("::*"):
        head = tree[: -3].strip()
        new_prefix = join_path(prefix, head) if prefix and head else (head or prefix)
        return [("*", new_prefix, True)]
    if tree == "*":
        return [("*", prefix, True)]
    if " as " in tree and "{" not in tree.split(" as ", 1)[0]:
        path, alias = tree.rsplit(" as ", 1)
        path = path.strip()
        alias = alias.strip()
        resolved = join_path(prefix, path) if prefix else path
        return [(alias, resolved, False)]
    brace = tree.find("::{")
    if brace >= 0:
        head = tree[:brace]
        rest = tree[brace + 2 :]
        new_prefix = join_path(prefix, head) if prefix else head
        return parse_use_tree(new_prefix, rest)
    if tree.startswith("{"):
        return parse_use_tree(prefix, tree)
    resolved = join_path(prefix, tree) if prefix else tree
    local = tree.split("::")[-1]
    return [(local, resolved, False)]


def iter_use_statements(src: str) -> Iterator[tuple[int, str, bool]]:
    """Yield (line, use-tree-text, is_reexport) for use / pub use items."""
    i = 0
    n = len(src)
    while i < n:
        m = re.search(r"(?:^|\n)[ \t]*(pub(?:\([^)]*\))?\s+)?use\s+", src[i:])
        if not m:
            break
        abs_start = i + m.start()
        line = src[: abs_start + m.end()].count("\n") + 1
        is_reexport = bool(m.group(1))
        j = i + m.end()
        depth = 0
        while j < n:
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
            elif src[j] == ";" and depth <= 0:
                tree = src[i + m.end() : j].strip()
                yield line, tree, is_reexport
                i = j + 1
                break
            j += 1
        else:
            break


def extract_type_aliases(src: str) -> list[tuple[int, str, str]]:
    rows: list[tuple[int, str, str]] = []
    for m in re.finditer(
        r"(?:^|\n)[ \t]*(?:pub(?:\([^)]*\))?\s+)?type\s+(\w+)\s*(?:<[^>]*>\s*)?=\s*([^;]+);",
        src,
    ):
        line = src[: m.start()].count("\n") + 1
        rows.append((line, m.group(1), re.sub(r"\s+", " ", m.group(2).strip())))
    return rows


def extract_structs(src: str) -> list[tuple[int, str, str]]:
    """Return (line, name, body) for struct { ... } and tuple struct ( ... ); items."""
    rows: list[tuple[int, str, str]] = []
    for m in re.finditer(
        r"(?:^|\n)[ \t]*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)\s*(?:<[^>]*>\s*)?\{",
        src,
    ):
        brace = m.end() - 1
        end = match_pairs(src, brace, "{", "}")
        if end < 0:
            continue
        line = src[: m.start()].count("\n") + 1
        rows.append((line, m.group(1), src[brace + 1 : end]))
    for m in re.finditer(
        r"(?:^|\n)[ \t]*(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)\s*(?:<[^>]*>\s*)?\(",
        src,
    ):
        paren = m.end() - 1
        end = match_pairs(src, paren, "(", ")")
        if end < 0:
            continue
        line = src[: m.start()].count("\n") + 1
        rows.append((line, m.group(1), src[paren + 1 : end]))
    return rows


def extract_impl_types(src: str) -> list[tuple[str, str]]:
    """Return (type_name, impl_body) for inherent impl blocks."""
    rows: list[tuple[str, str]] = []
    for m in re.finditer(
        r"(?:^|\n)[ \t]*impl\s+(?:<[^>]*>\s+)?(?:[\w:]+::)*(\w+)\s*(?:<[^>]*>\s*)?\{",
        src,
    ):
        brace = m.end() - 1
        end = match_pairs(src, brace, "{", "}")
        if end < 0:
            continue
        rows.append((m.group(1), src[brace + 1 : end]))
    return rows


def extract_functions(src: str) -> list[tuple[int, str, str, str]]:
    """Return (line, name, params, body) for fn items, including methods."""
    rows: list[tuple[int, str, str, str]] = []
    i = 0
    n = len(src)
    while True:
        m = re.search(
            r"(?:^|\n|[\{;])[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+(\w+)\s*",
            src[i:],
        )
        if not m:
            break
        name = m.group(1)
        abs_name_end = i + m.end()
        line = src[: i + m.start()].count("\n") + 1
        j = abs_name_end
        if j < n and src[j] == "<":
            gt = match_pairs(src, j, "<", ">")
            if gt < 0:
                i = abs_name_end
                continue
            j = gt + 1
        while j < n and src[j].isspace():
            j += 1
        if j >= n or src[j] != "(":
            i = abs_name_end
            continue
        close = match_pairs(src, j, "(", ")")
        if close < 0:
            i = abs_name_end
            continue
        params = src[j + 1 : close]
        k = close + 1
        while k < n and src[k].isspace():
            k += 1
        # skip -> ret and where ... until { or ;
        while k < n and src[k] not in "{;":
            if src[k] == "{":
                break
            k += 1
        if k >= n:
            break
        if src[k] == ";":
            rows.append((line, name, params, ""))
            i = k + 1
            continue
        if src[k] != "{":
            i = k + 1
            continue
        end = match_pairs(src, k, "{", "}")
        if end < 0:
            break
        rows.append((line, name, params, src[k + 1 : end]))
        i = end + 1
    return rows


def module_path_for(rel_path: str) -> str:
    rel = rel_path.replace("\\", "/")
    if rel.startswith("src/"):
        rel = rel[4:]
    if rel.endswith(".rs"):
        rel = rel[:-3]
    parts = rel.split("/")
    if parts[-1] == "mod":
        parts = parts[:-1]
    return "crate::" + "::".join(parts)


def line_of(src: str, index: int) -> int:
    return src[:index].count("\n") + 1
