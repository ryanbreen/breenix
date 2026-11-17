#!/usr/bin/env python3
"""Stand-in for breenfs-torture in scripts/test_breenfs_battery.py; never the real binary.

Takes the runner's arguments, writes a --progress file the way breenfs-torture
does and passes every seed except seed 3, which fails.
"""

import json
import os
import sys


def main() -> int:
    args = dict(zip(sys.argv[1::2], sys.argv[2::2]))
    seed = int(args["--seed-start"])
    progress = args["--progress"]
    failed = seed == 3
    state = "fail" if failed else "pass"
    record = {"seeds": [{"seed": seed, "state": state, "images": 100 * seed, "failures": int(failed)}],
              "images": 100 * seed, "failures": int(failed)}
    with open(progress + ".tmp", "w", encoding="utf-8") as out:
        json.dump(record, out)
    os.replace(progress + ".tmp", progress)
    print(f"seed={seed} images_checked={100 * seed} failures={int(failed)}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
