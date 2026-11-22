#!/usr/bin/env python3
"""Write every elfcheck fixture and print a manifest of them.

Usage: write_fixtures.py DIRECTORY

Writes fixtures.py's baseline object and controls into DIRECTORY/relocatable
and fixtures_exec.py's baseline executable and controls into DIRECTORY/exec,
through each module's own write(), then prints one tab-separated record per
line on stdout:

    check     ID                       every check id, fixtures.CHECKS then fixtures_exec.CHECKS
    baseline  MODE  PATH               MODE is "relocatable" or "exec"
    control   MODE  NAME  PATH  IDS    IDS is the control's expected check ids, comma-separated

The Rust test elfcheck::verdicts_equal_c_elfcheck reads it to run the C and
the Rust elfcheck over the same files. The toolchain workspace takes no
dependencies, so the format is plain lines rather than JSON. Nothing here
changes a fixture or an expectation; it only lists what the two modules
already define.
"""

from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import fixtures  # noqa: E402
import fixtures_exec  # noqa: E402


def records(mode, module, directory):
    baseline, paths = module.write(directory)
    yield ("baseline", mode, str(baseline))
    for name, path in paths.items():
        yield ("control", mode, name, str(path), ",".join(sorted(module.CONTROLS[name][0])))


def main():
    if len(sys.argv) != 2:
        sys.stderr.write("usage: write_fixtures.py DIRECTORY\n")
        return 2
    root = Path(sys.argv[1])
    lines = [("check", check) for check in fixtures.CHECKS + fixtures_exec.CHECKS]
    lines += records("relocatable", fixtures, root / "relocatable")
    lines += records("exec", fixtures_exec, root / "exec")
    for fields in lines:
        if any("\t" in field or "\n" in field for field in fields):
            sys.stderr.write(f"write_fixtures.py: a field holds a tab or newline: {fields!r}\n")
            return 1
        sys.stdout.write("\t".join(fields) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
