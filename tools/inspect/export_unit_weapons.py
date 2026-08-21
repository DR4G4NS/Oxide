#!/usr/bin/env python3
"""Convierte la salida de tools/inspect/InspectUnitWeapons.java en src/game/unit_weapons.tsv.

Uso: python3 export_unit_weapons.py <salida_de_InspectUnitWeapons.txt>
"""
import re
import sys

HEADER = ("# unit_id\tunit_name\tweapon\treload\tshots\tbullet_id\tspeed"
          "\tdamage\tlifetime\tsplash_damage\tsplash_radius\tpierce"
          "\tpierce_building\tstatus_id\tstatus_duration")

UNIT_RE = re.compile(
    r"(\S+) id=(\d+) class=(\S+) health=([\d.]+) speed=([\d.]+) "
    r"range=([\d.-]+) mounts=(\d+)"
)
WEAPON_RE = re.compile(
    r"(\S+) weapon=(\S*) reload=([\d.]+) shots=(\d+) inaccuracy=([\d.]+) "
    r"velocityRnd=([\d.]+) bullet=(\d+) speed=([\d.]+) damage=([\d.]+) "
    r"life=([\d.]+) splash=([\d.]+) radius=([\d.-]+) pierce=(\S+) "
    r"pierceBuilding=(\S+) cap=(\S+) homingRange=([\d.]+) status=(\S+)\((\d+)\) "
    r"statusDuration=([\d.]+) frags=(\d+) frag=(\S+)"
)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rows = [HEADER]
    unit = None
    for line in open(sys.argv[1], encoding="utf-8"):
        m = UNIT_RE.match(line)
        if m:
            unit = (int(m.group(2)), m.group(1))
            continue
        w = WEAPON_RE.match(line)
        if w and unit is not None:
            rows.append(
                "\t".join(
                    str(x)
                    for x in [
                        unit[0], unit[1], w.group(2), w.group(3), w.group(4),
                        w.group(7), w.group(8), w.group(9), w.group(10),
                        w.group(11), w.group(12), w.group(13), w.group(14),
                        w.group(18), w.group(19),
                    ]
                )
            )
    sys.stdout.write("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
