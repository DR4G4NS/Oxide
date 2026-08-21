#!/usr/bin/env python3
"""Regenerate src/game/block_power.tsv from the official server JAR.

Usage:
    javac -cp $JAR tools/inspect/InspectBlockPower.java
    java -cp tools/inspect:$JAR InspectBlockPower 2>/dev/null | python3 tools/inspect/export_block_power.py > src/game/block_power.tsv

The 2>/dev/null drops the JVM's ANSI-coloured "Preset ... doesn't have a sector
assigned." warnings that otherwise pollute the TSV header.
"""
import csv
import sys

rows = list(csv.reader(sys.stdin, delimiter='\t'))
header = next(r for r in rows if r and r[0].startswith('#'))
data = [r for r in rows if r and not r[0].startswith('#')]
out = csv.writer(sys.stdout, delimiter='\t', lineterminator='\n')
out.writerow(header)
for r in data:
    if len(r) == 11 and r[0].isdigit():
        out.writerow(r)
