#!/usr/bin/env python3
"""Blank every core-proof macro invocation, preserving line and column count.

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

Rung 2 (Component C) added `#[cfg(...)]` guards directly above several seam
invocations in `scheduler.rs` -- Component A's nine pre-existing `proof_point!`
calls each needed a `not(coreproof_component_c)` guard (a Component C build
compiles a different `SiteId` that has no matching variant), and M7's new
`proof_cover!(MaskedLockBare)` and the new `ScheduleEntry` seam are each gated
on their own feature. Blanking ONLY the macro-call text and leaving such a
guard attribute in place produces one of two failures once the file is
recompiled, independent of the blanking script's own correctness for the
*unguarded* case it was originally written against:

* if the blanked call was the last statement in its block, the attribute is
  left with nothing to attach to at all -- "expected statement after outer
  attribute", a hard parse error;
* otherwise the attribute silently reattaches itself to whatever REAL
  statement happens to follow. That is a correctness bug even where it
  happens to still parse (`#[cfg(...)]` on a bare, non-macro expression
  statement additionally hits E0658, "attributes on expressions are
  experimental", on this toolchain), because it can now gate a statement the
  attribute was never meant to guard.

So a `#[cfg(...)]` immediately guarding a seam call is blanked TOGETHER with
that call -- same rule as the macro text itself: same length, same line,
nothing shifts. Only guards that are themselves coreproof-specific are
touched (the pattern requires "coreproof" inside the `cfg(...)`), so an
unrelated attribute that happens to sit next to a seam for some other reason
is left alone. A seam with no immediately-preceding guard is unaffected by
this pass and is still handled by the plain, original rule below it.
"""

import os
import re
import sys

SEAM = re.compile(r"(crate::)?proof_(?:point|cover)!\([A-Za-z0-9_]*\);")

# A coreproof-specific `#[cfg(...)]` line immediately (only whitespace/newline
# between) guarding a seam invocation. Group 1 is the attribute text alone
# (blanked to spaces); group 2 is the whitespace/newline/indentation between
# the attribute and the call (left untouched -- it carries no code); group 3
# is the seam invocation itself (blanked to spaces, same rule as SEAM above).
GUARDED_SEAM = re.compile(
    r"(#\[cfg\([^\n]*coreproof[^\n]*\)\])([ \t]*\n[ \t]*)"
    r"((?:crate::)?proof_(?:point|cover)!\([A-Za-z0-9_]*\);)"
)


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

            def blank_guarded(m: "re.Match[str]") -> str:
                return " " * len(m.group(1)) + m.group(2) + " " * len(m.group(3))

            text, guarded_count = GUARDED_SEAM.subn(blank_guarded, text)
            replaced, bare_count = SEAM.subn(lambda m: " " * len(m.group(0)), text)
            count = guarded_count + bare_count
            if count:
                blanked += count
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write(replaced)
    print(blanked)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
