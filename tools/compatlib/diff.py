"""Differential comparison of two compat artifact trees."""

from __future__ import annotations

import json
from pathlib import Path


def load_wrapped(path: Path, key: str | None = None):
    doc = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(doc, list):
        return doc
    if key and isinstance(doc, dict) and key in doc:
        return doc[key]
    return doc


def load_build(compat_root: Path, build: str) -> dict:
    d = compat_root / build
    return {
        "manifest": load_wrapped(d / "manifest.json"),
        "packets": load_wrapped(d / "packets.json", "packets"),
        "streams": _optional(d / "streams.json", "streams"),
        "rpc": _optional(d / "rpc.json", "rpc"),
        "typeio": _optional(d / "typeio.json", "typeio"),
        "saves": load_wrapped(d / "saves.json", "saves"),
        "content": load_wrapped(d / "content.json", "content"),
        "entities": _optional(d / "entities.json", "entities"),
        "entity_sync": _optional(d / "entity-sync.json", "entity_sync"),
        "rules": load_wrapped(d / "rules.json", "rules"),
        "logic": load_wrapped(d / "logic.json", "logic"),
        "fingerprints": _optional(d / "semantic-fingerprints.json", "fingerprints"),
    }


def _optional(path: Path, key: str):
    if not path.exists():
        return None
    return load_wrapped(path, key)


def _by_name(items, name_key="name"):
    if not items:
        return {}
    if isinstance(items, dict) and name_key not in items:
        return items
    return {it[name_key]: it for it in items if isinstance(it, dict) and name_key in it}


def _by_id(items, id_key="id"):
    if not items:
        return {}
    return {it[id_key]: it for it in items if isinstance(it, dict) and id_key in it}


def diff_named(from_items, to_items, name_key="name") -> dict:
    a = _by_name(from_items, name_key)
    b = _by_name(to_items, name_key)
    added = [b[n] for n in b if n not in a]
    removed = [a[n] for n in a if n not in b]
    changed = []
    for n in a:
        if n in b and a[n] != b[n]:
            changed.append({"name": n, "from": a[n], "to": b[n]})
    return {"added": added, "removed": removed, "changed": changed}


def diff_packets(from_pkts, to_pkts) -> dict:
    named = diff_named(from_pkts, to_pkts)
    from_ids = _by_id(from_pkts)
    to_ids = _by_id(to_pkts)
    from_by_name = _by_name(from_pkts)
    to_by_name = _by_name(to_pkts)
    shifted = []
    renamed = []
    order_changed = False
    from_order = [p.get("name") for p in from_pkts or []]
    to_order = [p.get("name") for p in to_pkts or []]
    shared = [n for n in from_order if n in to_by_name]
    shared_to = [n for n in to_order if n in from_by_name]
    if shared != shared_to:
        order_changed = True
    for name, fp in from_by_name.items():
        if name in to_by_name and fp.get("id") != to_by_name[name].get("id"):
            shifted.append(
                {
                    "name": name,
                    "from_id": fp.get("id"),
                    "to_id": to_by_name[name].get("id"),
                }
            )
    for i in sorted(set(from_ids) & set(to_ids)):
        if from_ids[i].get("name") != to_ids[i].get("name"):
            renamed.append(
                {
                    "id": i,
                    "from": from_ids[i].get("name"),
                    "to": to_ids[i].get("name"),
                }
            )
    named["id_shifted"] = shifted
    named["class_or_id_renamed"] = renamed
    named["registration_order_changed"] = order_changed
    return named


def _logic_access(logic):
    if isinstance(logic, list):
        return logic
    if isinstance(logic, dict):
        return logic.get("access") or logic.get("logic_access") or logic.get("laccess") or []
    return []


def diff_builds(from_data: dict, to_data: dict) -> dict:
    content_from = from_data["content"] if isinstance(from_data["content"], dict) else {}
    content_to = to_data["content"] if isinstance(to_data["content"], dict) else {}
    saves_from = from_data["saves"]
    saves_to = to_data["saves"]
    if isinstance(saves_from, dict):
        saves_from = saves_from.get("versions") or saves_from.get("saves") or saves_from
    if isinstance(saves_to, dict):
        saves_to = saves_to.get("versions") or saves_to.get("saves") or saves_to

    from_save_map = {s["version"]: s for s in saves_from}
    to_save_map = {s["version"]: s for s in saves_to}

    delta = {
        "packets": diff_packets(from_data["packets"], to_data["packets"]),
        "streams": diff_named(
            from_data.get("streams") or [],
            to_data.get("streams") or [],
            name_key="class_name",
        ),
        "rpc": diff_named(
            from_data.get("rpc") or [],
            to_data.get("rpc") or [],
            name_key="generated_class",
        ),
        "typeio": diff_named(
            (from_data.get("typeio") or {}).get("methods")
            if isinstance(from_data.get("typeio"), dict)
            else from_data.get("typeio") or [],
            (to_data.get("typeio") or {}).get("methods")
            if isinstance(to_data.get("typeio"), dict)
            else to_data.get("typeio") or [],
        ),
        "saves": {
            "added": [to_save_map[v] for v in to_save_map if v not in from_save_map],
            "removed": [from_save_map[v] for v in from_save_map if v not in to_save_map],
            "class_changed": [
                {
                    "version": v,
                    "from": from_save_map[v].get("class"),
                    "to": to_save_map[v].get("class"),
                }
                for v in from_save_map
                if v in to_save_map and from_save_map[v].get("class") != to_save_map[v].get("class")
            ],
            "current_writer_from": max(from_save_map) if from_save_map else None,
            "current_writer_to": max(to_save_map) if to_save_map else None,
        },
        "content": {
            key: diff_named(content_from.get(key) or [], content_to.get(key) or [])
            for key in (
                "items",
                "liquids",
                "weathers",
                "status_effects",
                "blocks",
                "units",
                "unit_commands",
                "unit_stances",
            )
        },
        "rules": diff_named(from_data["rules"], to_data["rules"]),
        "logic": {
            "access": diff_named(_logic_access(from_data["logic"]), _logic_access(to_data["logic"])),
        },
        "entities": diff_named(from_data.get("entities") or [], to_data.get("entities") or []),
        "entity_sync": {
            "from": from_data.get("entity_sync"),
            "to": to_data.get("entity_sync"),
            "changed": (from_data.get("entity_sync") or {}) != (to_data.get("entity_sync") or {}),
        },
        "fingerprints": {
            "from": from_data.get("fingerprints"),
            "to": to_data.get("fingerprints"),
            "changed": (from_data.get("fingerprints") or {}) != (to_data.get("fingerprints") or {}),
        },
    }

    typeio_from = from_data.get("typeio") or {}
    typeio_to = to_data.get("typeio") or {}
    if isinstance(typeio_from, dict) and isinstance(typeio_to, dict):
        tags_from = typeio_from.get("object_tags") or []
        tags_to = typeio_to.get("object_tags") or []
        delta["typeio_tags"] = diff_named(tags_from, tags_to, name_key="tag" if tags_to and "tag" in (tags_to[0] or {}) else "name")

    return delta
