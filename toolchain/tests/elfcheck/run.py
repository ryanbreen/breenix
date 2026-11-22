#!/usr/bin/env python3
"""Build elfcheck and score it against its hand-built controls.

Usage: run.py --clang /absolute/path/to/clang --lld /absolute/path/to/lld
              [--host-arch ARCH ...]

The clang named here builds elfcheck under -std=c11 -Wall -Wextra -Werror
-pedantic, once plainly and once under ASan and UBSan, and cross-compiles the
black-box C and C++ sources to x86-64 ELF relocatable objects. The lld named
here links blackbox/program.c's objects through userspace/programs/linker.ld
into static executables, which `elfcheck --exec` must accept. On macOS each
--host-arch (arm64, x86_64) adds a strict build for that architecture whose
binary runs every control as well; Rosetta runs the x86_64 binary on arm64.

Results print one line per check: `elfcheck::<name> PASS` or `FAIL`.
"""

import argparse
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
import fixtures  # noqa: E402
import fixtures_exec  # noqa: E402

STRICT = ["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"]
SANITIZE = ["-O1", "-g", "-fsanitize=address,undefined", "-fno-sanitize-recover=all"]
FINDING = re.compile(r"^(?P<file>.+): (?P<check>(?:ehdr|shdr|strtab|symtab|sym|rela|group|limit"
                     r"|phdr|load|tls)\.[a-z0-9-]+): ")
ALL_CHECKS = fixtures.CHECKS + fixtures_exec.CHECKS
BLACKBOX = [
    ("objects.c", ["-O0"]),
    ("objects.c", ["-O2", "-fPIC"]),
    ("objects.c", ["-O0", "-g", "-fcommon"]),
    ("objects.c", ["-O2", "-ffunction-sections", "-fdata-sections"]),
    ("comdat.cc", ["-O0", "-fno-exceptions"]),
]
# program.c compile flags, then extra lld flags; each link uses linker.ld.
BLACKBOX_EXEC = [
    (["-O0"], []),
    (["-O2"], []),
    (["-O2", "-fPIE"], []),
    (["-O2", "-ffunction-sections", "-fdata-sections"], ["--gc-sections"]),
]
LINKER_SCRIPT = ROOT / "userspace" / "programs" / "linker.ld"


class Runner:
    def __init__(self, clang, scratch):
        self.clang, self.scratch, self.failures = clang, scratch, 0

    def result(self, name, ok, detail=""):
        print(f"elfcheck::{name} {'PASS' if ok else 'FAIL'}" + (f" ({detail})" if detail and not ok else ""))
        if not ok:
            self.failures += 1
        return ok

    def compile(self, output, flags):
        command = [str(self.clang), *STRICT, *flags, "-o", str(output), str(HERE / "elfcheck.c")]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        return result.returncode == 0 and not result.stderr.strip(), result.stderr.strip()

    def findings(self, binary, path, mode=()):
        result = subprocess.run([*binary, *mode, str(path)], capture_output=True, text=True, timeout=10)
        found = set()
        for line in result.stdout.splitlines():
            match = FINDING.match(line)
            if match and match.group("file") == str(path):
                found.add(match.group("check"))
        return result, found


def score_controls(runner, label, binary, mode, baseline, controls, table):
    prefix = "exec_" if mode else ""
    result, found = runner.findings(binary, baseline, mode)
    runner.result(f"{prefix}baseline_passes[{label}]", result.returncode == 0 and not found,
                  f"exit {result.returncode}, findings {sorted(found)}, stderr {result.stderr.strip()!r}")

    for name, path in controls.items():
        expected = table[name][0]
        result, found = runner.findings(binary, path, mode)
        runner.result(f"{prefix}control[{label}][{name}]", result.returncode == 1 and found == expected,
                      f"exit {result.returncode}, expected {sorted(expected)}, found {sorted(found)}"
                      + (f", stderr {result.stderr.strip()!r}" if result.stderr.strip() else ""))


def score(runner, label, binary, rel_set, exec_set):
    listing = subprocess.run([*binary, "--list"], capture_output=True, text=True, timeout=10)
    listed = listing.stdout.split()
    runner.result(f"check_ids_match[{label}]",
                  listing.returncode == 0 and sorted(listed) == sorted(ALL_CHECKS)
                  and len(set(listed)) == len(listed),
                  f"elfcheck --list differs from the fixtures' CHECKS: {sorted(set(listed) ^ set(ALL_CHECKS))}")

    score_controls(runner, label, binary, (), *rel_set, fixtures.CONTROLS)
    score_controls(runner, label, binary, ("--exec",), *exec_set, fixtures_exec.CONTROLS)

    missing = runner.scratch / "absent.o"
    result = subprocess.run([*binary, str(missing)], capture_output=True, text=True, timeout=10)
    runner.result(f"unreadable_file_is_an_error[{label}]", result.returncode == 2 and str(missing) in result.stderr,
                  f"exit {result.returncode}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--clang", type=Path, required=True)
    parser.add_argument("--lld", type=Path, required=True)
    parser.add_argument("--host-arch", action="append", default=[], choices=["arm64", "x86_64"])
    options = parser.parse_args()
    for flag, tool in (("--clang", options.clang), ("--lld", options.lld)):
        if not tool.is_absolute() or not os.access(tool, os.X_OK):
            parser.error(f"{flag} requires an absolute executable path")
    if options.host_arch and platform.system() != "Darwin":
        parser.error("--host-arch uses the macOS -arch driver option")
    # The generic lld driver needs its ELF flavor named; an ld.lld name selects it.
    lld = [str(options.lld)] + (["-flavor", "gnu"] if options.lld.name in {"lld", "rust-lld"} else [])

    with tempfile.TemporaryDirectory(prefix="elfcheck.") as scratch:
        scratch = Path(scratch)
        runner = Runner(options.clang, scratch)

        expected = [e for e, _ in fixtures.CONTROLS.values()] + [e for e, _ in fixtures_exec.CONTROLS.values()]
        uncovered = set(ALL_CHECKS) - set().union(*expected)
        unknown = set().union(*expected) - set(ALL_CHECKS)
        runner.result("every_check_has_a_control", not uncovered and not unknown,
                      f"uncovered {sorted(uncovered)}, unknown {sorted(unknown)}")

        rel_set = fixtures.write(scratch / "controls")
        exec_set = fixtures_exec.write(scratch / "exec-controls")

        binaries = []
        ok, stderr = runner.compile(scratch / "elfcheck", ["-O2"])
        if runner.result("builds_strict[native]", ok, stderr):
            binaries.append(("native", [str(scratch / "elfcheck")]))
        ok, stderr = runner.compile(scratch / "elfcheck-san", SANITIZE)
        if runner.result("builds_strict[asan+ubsan]", ok, stderr):
            binaries.append(("asan+ubsan", [str(scratch / "elfcheck-san")]))
        for arch in options.host_arch:
            output = scratch / f"elfcheck-{arch}"
            ok, stderr = runner.compile(output, ["-O2", "-arch", arch])
            if runner.result(f"builds_strict[{arch}]", ok, stderr):
                launcher = ["/usr/bin/arch", f"-{arch}"]
                binaries.append((arch, [*launcher, str(output)]))

        for label, binary in binaries:
            score(runner, label, binary, rel_set, exec_set)

        if binaries:
            # Real compiler output runs through the sanitized build when it exists.
            label, binary = next((b for b in binaries if b[0] == "asan+ubsan"), binaries[0])
            for source, flags in BLACKBOX:
                output = scratch / f"{source}{''.join(flags)}.o"
                command = [str(options.clang), "--target=x86_64-unknown-linux-gnu", "-ffreestanding",
                           "-nostdinc", "-fno-addrsig", *flags, "-c", "-o", str(output),
                           str(HERE / "blackbox" / source)]
                built = subprocess.run(command, capture_output=True, text=True, timeout=30)
                name = f"clang_object_passes[{label}][{source} {' '.join(flags)}]"
                if not runner.result(name + "[compiles]", built.returncode == 0, built.stderr.strip()):
                    continue
                result, found = runner.findings(binary, output)
                runner.result(name, result.returncode == 0 and not found,
                              f"exit {result.returncode}, findings {sorted(found)}: {result.stdout.strip()}")

            for flags, link_flags in BLACKBOX_EXEC:
                tag = "".join(flags + link_flags)
                obj, exe = scratch / f"program{tag}.o", scratch / f"program{tag}"
                name = f"lld_executable_passes[{label}][program.c {' '.join(flags + link_flags)}]"
                built = subprocess.run(
                    [str(options.clang), "--target=x86_64-unknown-linux-gnu", "-ffreestanding", "-nostdinc",
                     "-fno-addrsig", "-mno-red-zone", *flags, "-c", "-o", str(obj),
                     str(HERE / "blackbox" / "program.c")],
                    capture_output=True, text=True, timeout=30)
                if not runner.result(name + "[compiles]", built.returncode == 0, built.stderr.strip()):
                    continue
                linked = subprocess.run([*lld, "-static", "-T", str(LINKER_SCRIPT), *link_flags,
                                         "-o", str(exe), str(obj)],
                                        capture_output=True, text=True, timeout=30)
                if not runner.result(name + "[links]", linked.returncode == 0, linked.stderr.strip()):
                    continue
                result, found = runner.findings(binary, exe, ("--exec",))
                runner.result(name, result.returncode == 0 and not found,
                              f"exit {result.returncode}, findings {sorted(found)}: {result.stdout.strip()}")

    total = "PASS" if runner.failures == 0 else f"FAIL ({runner.failures} failed)"
    print(f"elfcheck: {total}")
    return 0 if runner.failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
