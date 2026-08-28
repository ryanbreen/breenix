#!/usr/bin/env python3
"""Blank every `proof_point!` invocation in a tree, preserving line and column count.

LEG 3 of the core-proof production-cleanliness ratchet compares the production
`.text` built with the seams present against the production `.text` built with
them neutralised. That comparison is only about the seams if nothing ELSE moves
between the two builds, and two things otherwise do:

* Deleting a seam line shifts every following line number, which moves the
  `core::panic::Location` records those lines feed. `.text` addresses those
  records, so a delete reddens the leg on a tree whose seams genuinely cost
  nothing. Blanking in place — same line, same column count — moves nothing.
* An absolute path difference changes embedded path strings, so the caller must
  run both builds from the same directory.

Prints the number of invocations blanked. The caller treats zero as a failure:
two identical builds prove nothing.
"""

import os
import re
import sys

SEAM = re.compile(r"(crate::)?proof_point!\([A-Za-z0-9_]*\);")


def main() -> int:
    root = sys.argv[1]
    blanked = 0
    for dirpath, _dirnames, filenames in os.walk(os.path.join(root, "kernel", "src")):
        for name in filenames:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            with open(path, encoding="utf-8") as handle:
                text = handle.read()
            replaced, count = SEAM.subn(lambda m: " " * len(m.group(0)), text)
            if count:
                blanked += count
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write(replaced)
    print(blanked)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
