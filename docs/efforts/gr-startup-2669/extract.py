#!/usr/bin/env python3
"""Extract the eight saved GR startup captures; exit nonzero on a mismatch."""
import json
from pathlib import Path
import re

CAPTURES = (
    ("20261106T190545Z", 1), ("20261106T211848Z", 1),
    ("20261106T232244Z", 1), ("20261102T102648Z", 1),
    ("20261106T152520Z", 0), ("20261106T173853Z", 0),
    ("20261106T231102Z", 0), ("20261102T110818Z", 0),
)
REGISTERS = (0x40000, 0x40014, 0x40028, 0x40120, 0x2640, 0x2644, 0x800000, 0x800004)
ROOT = Path.home() / "Library/Application Support/Vigil/breenix/boots"


def main():
    for capture, expected in CAPTURES:
        lines = (ROOT / f"{capture}-metal" / "serial.txt").read_text(errors="strict").splitlines()
        order, snapshots, timings = [], [], []
        for number, line in enumerate(lines, 1):
            match = re.search(r"\b(GT710_GR_[A-Z0-9_]+)\b", line)
            if match:
                label = match.group(1)
                if label == "GT710_GR_IRQ":
                    snapshot = json.loads(line[match.end():].strip())
                    snapshots.append(snapshot)
                    label += f"({snapshot['phase']})"
                order.append(f"{number}:{label}")
            if "GT710_PROBE_TIME graphics " in line:
                timings.append(f"{number}:{line[line.index('GT710_PROBE_TIME graphics '):]}")
        if not snapshots or snapshots[-1]["phase"] != "after" or len(timings) != 1:
            raise ValueError(f"{capture}: missing final after snapshot or unique graphics time")
        values = dict(snapshots[-1]["registers"])
        missing = set(REGISTERS) - values.keys()
        if missing:
            raise ValueError(f"{capture}: missing registers {sorted(missing)}")
        refusal = any(":GT710_GR_REFUSAL" in item for item in order)
        if refusal != bool(expected) or values[0x40120] != expected:
            raise ValueError(f"{capture}: unexpected refusal or 0x40120={values[0x40120]:#x}")
        print(f"{capture}-metal {'refusal' if expected else 'clean'}")
        print("  order: " + " -> ".join(order))
        print("  " + timings[0])
        print("  last IRQ (after): " + " ".join(f"{address:#x}={values[address]:#x}" for address in REGISTERS))


if __name__ == "__main__":
    main()
