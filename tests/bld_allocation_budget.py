#!/usr/bin/env python3
"""Measure each x86 reference link with Breenix's allocator on a Unix build host.

Pass the sealed reference .o directory and the x86 C-ABI libc.a used for the link.
The Rust probe routes bld allocations through the in-tree malloc and counts host
mmap calls from that allocator. It does not run the resulting guest ELFs.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT / "libs/libbreenix-libc/src/stdlib"
# Per fresh bld process. These budgets leave headroom for corpus and libc.a
# changes while keeping mappings two orders of magnitude below allocation calls.
BUDGET = (100_000, 1_000, 600)


def build_probe(tmp):
    target = tmp / "target"
    subprocess.run(
        ["cargo", "build", "--offline", "--locked", "--release", "-p", "bld",
         "--lib", "--target-dir", str(target)],
        cwd=ROOT / "toolchain", check=True,
    )
    stdlib = tmp / "stdlib"
    stdlib.mkdir()
    for source in ("malloc.rs", "alloc_cache.rs"):
        # Keep host libc's exported symbols separate from the allocator under test.
        text = (SOURCES / source).read_text().replace("#[no_mangle]", "")
        (stdlib / source).write_text(text)
    probe = tmp / "bld_allocation_probe.rs"
    shutil.copyfile(ROOT / "tests/fixtures" / probe.name, probe)
    binary = tmp / "bld-allocation-probe"
    release = target / "release"
    subprocess.run(
        ["rustc", "--edition=2021", "-Dwarnings", "-Copt-level=3", "--extern",
         f"bld={release / 'libbld.rlib'}", "-L", f"dependency={release / 'deps'}",
         str(probe), "-o", str(binary)],
        cwd=ROOT / "toolchain", check=True,
    )
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("objects", type=Path)
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    names = (ROOT / "toolchain/corpus/cases").read_text().splitlines()
    if len(names) != 30 or len(set(names)) != 30:
        raise ValueError("expected the 30 distinct reference programs")
    script = ROOT / "userspace/programs/linker.ld"
    totals = [0, 0, 0]
    peaks = [0, 0, 0]
    over_budget = []
    with tempfile.TemporaryDirectory(prefix="bld-allocation-") as directory:
        tmp = Path(directory)
        probe = build_probe(tmp)
        for name in names:
            out = tmp / f"{name}.elf"
            result = subprocess.run(
                [probe, "-o", out, "-T", script, "--gc-sections",
                 args.objects / f"{name}.o", args.archive],
                capture_output=True, text=True, timeout=10,
            )
            if result.returncode:
                raise RuntimeError(f"{name}: bld exited {result.returncode}\n{result.stderr}")
            with out.open("rb") as elf:
                if elf.read(4) != b"\x7fELF":
                    raise ValueError(f"{name}: bld output is not an ELF")
            allocations, maps, peak_live = map(int, result.stderr.split())
            counts = (allocations, maps, peak_live)
            print(f"{name}: allocations={allocations} mmap_calls={maps} peak_live={peak_live}")
            totals = [a + b for a, b in zip(totals, counts)]
            peaks = [max(a, b) for a, b in zip(peaks, counts)]
            if not 0 < peak_live <= maps or any(n > limit for n, limit in zip(counts, BUDGET)):
                over_budget.append(name)
    print(f"30 links: total allocations={totals[0]} mmap_calls={totals[1]}")
    print(f"Per link max (allocations, mmap calls, live mappings): {tuple(peaks)}; limits {BUDGET}")
    if over_budget:
        sys.exit("Allocation budget exceeded: " + ", ".join(over_budget))


if __name__ == "__main__":
    os.environ.setdefault("CARGO_BUILD_JOBS", "4")
    main()
