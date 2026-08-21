#!/usr/bin/env python3
"""Convierte la salida de tools/inspect/InspectStatusEffects.java en src/game/status_effects.tsv.

Uso: python3 tools/inspect/export_status_effects.py <salida_de_InspectStatusEffects.txt>
"""
import re
import sys

HEADER = ("# id\tname\tspeed\tdamageMult\treloadMult\thealthMult"
          "\tdamagePerTick\tpermanent")

ROW_RE = re.compile(
    r"(\d+)\t(\S+)\t([\d.]+)\t([\d.]+)\t([\d.]+)\t([\d.]+|Infinity)"
    r"\t(-?[\d.]+)\t([\d.]+)\t([\d.]+)\t(\S+)\t(\S+)\t([\d.]+)"
    r"\t([\d.]+)\t(\S+)"
)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rows = [HEADER]
    for line in open(sys.argv[1], encoding="utf-8"):
        m = ROW_RE.match(line)
        if m:
            rows.append(
                "\t".join(
                    [
                        m.group(1),  # id
                        m.group(2),  # name
                        m.group(3),  # speed
                        m.group(4),  # damageMult
                        m.group(5),  # reloadMult
                        m.group(6),  # healthMult
                        m.group(7),  # damagePerTick
                        m.group(10),  # permanent
                    ]
                )
            )
    sys.stdout.write("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
