#!/usr/bin/env python3
"""Compare Rust bld with the in-tree C bld during 25.R (ruling 17, O9)."""

import argparse
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True

from run import STRICT, TOOLCHAIN, archive_bytes, require

ROOT = TOOLCHAIN.parent
CORPUS = TOOLCHAIN / "corpus"
CHECK = "bld_parity::links_equal_c_bld"


def object_inputs(work, clang):
    # Use the bundle's sealed C producer and the same four linker fixtures.
    sys.path.insert(0, str(ROOT / "scripts"))
    loader = SourceFileLoader("bundle_tools", str(ROOT / "scripts/toolchain-build-tools.py"))
    bundle = module_from_spec(spec_from_loader(loader.name, loader))
    loader.exec_module(bundle)
    objects = work / "objs"
    objects.mkdir()
    names = (CORPUS / "cases").read_text().splitlines()
    require(len(names) == len(set(names)) == 30, "expected 30 distinct reference objects")
    for name in names:
        obj, _, _ = bundle.reference.compile_c(
            "x86_64", [clang], CORPUS / f"{name}.c", (CORPUS,), work,
        )
        obj.rename(objects / f"{name}.o")
    bundle.build_linker_objects("x86_64", [clang], work, work)
    require(set(bundle.LINKER_SOURCES) == {
        "got-main.c", "got-entry.s", "archive-value-with-long-name.s", "tls-template.c",
    }, "unexpected linker fixture set")
    return names, objects


def links_equal_c_bld(bld, clang, libc):
    with tempfile.TemporaryDirectory(prefix="bld-parity-") as tmp:
        work = Path(tmp).resolve()
        c_bld = work / "c-bld"
        subprocess.run([clang, *STRICT, TOOLCHAIN / "bld/bld.c",
                        TOOLCHAIN / "bld/script.c", TOOLCHAIN / "common/ar.c",
                        "-o", c_bld], check=True, timeout=30)
        names, objects = object_inputs(work, clang)
        script = ROOT / "userspace/programs/linker.ld"
        got_main, got_entry, archive_value, tls_template = (
            objects / f"{name}.o" for name in
            ("got-main", "got-entry", "archive-value-with-long-name", "tls-template")
        )
        archive = work / "got.a"
        archive.write_bytes(archive_bytes([
            (got_entry.name, got_entry, "archive_entry"),
            (archive_value.name, archive_value, "archive_value"),
        ]))
        duplicate = work / "duplicate-entry.o"
        shutil.copyfile(got_entry, duplicate)

        def link(binary, output, inputs):
            return subprocess.run(
                [binary, "-T", script, "--gc-sections", "-o", output, *inputs, libc],
                capture_output=True, timeout=10,
            )

        accepted = [(name, (objects / f"{name}.o",)) for name in names]
        accepted += [("got-and-archives", (got_main, archive)),
                     ("tls-template", (tls_template,))]
        differences = []
        for name, inputs in accepted:
            expected, actual = (work / f"{name}-{p}.elf" for p in ("c", "rust"))
            c = link(c_bld, expected, inputs)
            rust = link(bld, actual, inputs)
            require(c.returncode == 0 and expected.is_file(),
                    f"C bld failed to link {name}: {c.stderr!r}")
            if rust.returncode != 0:
                differences.append(f"{name}: Rust bld rejected inputs: {rust.stderr!r}")
            elif not actual.is_file() or expected.read_bytes() != actual.read_bytes():
                differences.append(f"{name}: ELF bytes differ")
        print(f"ELF parity: {len(accepted) - len(differences)}/{len(accepted)}")

        rejected = (
            ("undefined", (got_main,), os.fsencode(got_main) + b": .text+0x",
             b"R_X86_64_PLT32 (4) archive_entry: undefined symbol"),
            ("duplicate", (got_main, got_entry, duplicate), os.fsencode(duplicate) + b": ",
             b"archive_entry: duplicate definition (first defined in "
             + os.fsencode(got_entry) + b")"),
        )
        diagnostic_failures = []
        for name, inputs, location, message in rejected:
            output = work / f"{name}.elf"
            c = link(c_bld, output, inputs)
            require(c.returncode == 1 and not output.exists() and any(
                line.startswith(location) and line.endswith(message)
                for line in c.stderr.splitlines()),
                f"C bld did not give a located rejection for {name}: {c.stderr!r}")
            rust = link(bld, output, inputs)
            if (c.returncode, c.stderr) != (rust.returncode, rust.stderr) or output.exists():
                diagnostic_failures.append(f"{name}: C ({c.returncode}, {c.stderr!r}), "
                                           f"Rust ({rust.returncode}, {rust.stderr!r})")
        print(f"Diagnostic parity: {len(rejected) - len(diagnostic_failures)}/{len(rejected)}")
        require(not differences and not diagnostic_failures,
                "\n".join(differences + diagnostic_failures))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bld", type=Path)
    parser.add_argument("--clang", default=shutil.which("clang"))
    parser.add_argument("--libc", type=Path, required=True, help="Breenix x86-64 C-ABI libc.a")
    args = parser.parse_args()
    try:
        links_equal_c_bld(args.bld.resolve(strict=True),
                          str(Path(args.clang).absolute()), args.libc.resolve(strict=True))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"{CHECK} FAIL\n{error}")
    print(f"PASS {CHECK}")
