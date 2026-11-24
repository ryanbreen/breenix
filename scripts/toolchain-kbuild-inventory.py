#!/usr/bin/env python3
"""Inventory the pinned BusyBox makefiles with Cargo-built host Rust tools."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "vendor/busybox"
WORKLOAD = "busybox-1.37.0"
INPUT_PINS = ROOT / "toolchain/docs/busybox-c-inventory.json"
OUTPUT = ROOT / "toolchain/docs/kbuild-make-inventory.json"

# GNU make 4.4.1, edition 0.77, as pinned in toolchain/docs/spec-ledger.md.
# Chapter, construct kind, implementation unit, and spellings in the manual.
CLASSIFICATIONS = (
    ("3 Writing Makefiles", "directive", "directives", "include -include sinclude"),
    ("4 Writing Rules", "directive", "unassigned", "vpath"),
    ("4 Writing Rules", "pattern-rule", "patterns-text", "static"),
    ("4 Writing Rules", "prerequisite", "directives", "order-only"),
    ("4 Writing Rules", "special-target", "directives", ".PHONY"),
    ("4 Writing Rules", "special-target", "control",
     ".DEFAULT .PRECIOUS .INTERMEDIATE .NOTINTERMEDIATE .SECONDARY "
     ".SECONDEXPANSION .DELETE_ON_ERROR .IGNORE .LOW_RESOLUTION_TIME .SILENT "
     ".EXPORT_ALL_VARIABLES .NOTPARALLEL .ONESHELL .POSIX"),
    ("5 Writing Recipes in Rules", "special-target", "control", ".WAIT"),
    ("6 How to Use Variables", "assignment", "directives", "= := ::= :::= += ?= !="),
    ("6 How to Use Variables", "directive", "directives", "define endef override private"),
    ("6 How to Use Variables", "directive", "control", "undefine"),
    ("6 How to Use Variables", "target-variable", "directives", "target-specific"),
    ("5 Writing Recipes in Rules", "directive", "directives", "export unexport"),
    ("7 Conditional Parts of Makefiles", "directive", "directives", "ifeq ifneq ifdef ifndef else endif"),
    ("8 Functions for Transforming Text", "function", "patterns-text",
     "subst patsubst strip findstring filter filter-out sort word wordlist words firstword "
     "lastword dir notdir suffix basename addsuffix addprefix join wildcard realpath abspath"),
    ("8 Functions for Transforming Text", "function", "control",
     "if or and intcmp let foreach file call value eval origin flavor error warning info shell"),
    ("8 Functions for Transforming Text", "function", "unassigned", "guile"),
    ("10 Using Implicit Rules", "pattern-rule", "patterns-text", "%"),
    ("10 Using Implicit Rules", "special-target", "patterns-text", ".SUFFIXES"),
    ("12 Extending GNU make", "directive", "unassigned", "load -load"),
)
CONSTRUCTS = {
    (kind, spelling): (chapter, unit)
    for chapter, kind, unit, spellings in CLASSIFICATIONS
    for spelling in spellings.split()
}
CONTROL_FUNCTIONS = set(
    "if and or foreach call value eval origin flavor shell error warning info file".split()
)
TOOL_BINDINGS = {"CC": "bcc", "HOSTCC": "bcc", "LD": "bld", "AR": "bar", "NM": "bnm"}
ENV = {"PATH": os.defpath, "LC_ALL": "C"}


def verify(path, expected):
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f"{path.name}: SHA-256 mismatch (expected {expected}, got {actual})")


def prepare_workload(directory):
    pins = json.loads(INPUT_PINS.read_text())["inputs"]
    archive = VENDOR / f"{WORKLOAD}.tar.bz2"
    config = VENDOR / "breenix-libc.config"
    series = VENDOR / "patches/series"
    for path, pin in ((archive, "archive"), (config, "config"), (series, "series")):
        verify(path, pins[pin])
    names = [line for line in series.read_text().splitlines() if line and not line.startswith("#")]
    if (len(names) != len(pins["patches"]) or set(names) != pins["patches"].keys()
            or any(not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or ".." in name for name in names)):
        raise ValueError("BusyBox patch series differs from its pinned inventory")
    for name in names:
        verify(series.parent / name, pins["patches"][name])

    with tarfile.open(archive, "r:bz2") as tar:
        tar.extractall(directory, filter="data")
    source = directory / WORKLOAD
    for name in names:
        with (series.parent / name).open("rb") as patch:
            subprocess.run(["patch", "-f", "-p1"], cwd=source, env=ENV, stdin=patch,
                           capture_output=True, text=True, timeout=60, check=True)
    shutil.copyfile(config, source / ".config")

    # Kbuild reads the generated Kbuild files rather than their .src templates.
    # Run the pinned workload's generator as a black box in the extracted tree.
    subprocess.run(["/bin/sh", "scripts/gen_build_files.sh", ".", "."], cwd=source,
                   env=ENV, capture_output=True, text=True, timeout=60, check=True)
    for template in source.rglob("Kbuild.src"):
        if not template.with_suffix("").is_file():
            raise ValueError(f"missing generated makefile for {template.relative_to(source)}")
    return source, pins


def makefiles(source):
    if not (source / "Makefile").is_file():
        raise ValueError("the BusyBox fixture has no top-level Makefile")
    paths = {"Makefile"}
    for directory, _, names in os.walk(source):
        directory = Path(directory).relative_to(source)
        support = not directory.parts or directory.parts[0] == "scripts"
        if "Kbuild" in names:
            paths.add((directory / "Kbuild").as_posix())
        elif "Makefile" in names:
            paths.add((directory / "Makefile").as_posix())
        for name in names:
            if name.endswith(".include") or (support and name.startswith("Makefile.")):
                paths.add((directory / name).as_posix())
    return sorted(paths)


def inventory(source, files, bmake):
    args = [str(bmake), "--inventory", *(arg for path in files for arg in ("-f", path))]
    result = subprocess.run(args, cwd=source, env=ENV, capture_output=True, text=True,
                            timeout=60, check=True)
    known_files = set(files)
    occurrences = {}
    for row in result.stdout.splitlines():
        location, kind, name = row.split("\t")
        path, line = location.rsplit(":", 1)
        if path not in known_files or not line.isdecimal() or int(line) < 1:
            raise ValueError(f"invalid bmake inventory location: {location}")
        occurrences.setdefault((kind, name), []).append(location)
    if not occurrences:
        raise ValueError("bmake emitted an empty inventory for Kbuild")

    chapters = {}
    for (kind, name), locations in sorted(occurrences.items()):
        chapter, unit = CONSTRUCTS.get((kind, name), ("Unclassified", "unassigned"))
        construct = {"kind": kind, "construct": name, "unit": unit}
        construct["supported"] = (
            unit in ("directives", "patterns-text")
            or (kind == "function" and name in CONTROL_FUNCTIONS)
        )
        construct["locations"] = locations
        chapters.setdefault(chapter, []).append(construct)
    return dict(sorted(chapters.items(), key=lambda item: int(item[0].split()[0])
                       if item[0] != "Unclassified" else float("inf")))


def compile_outputs(stdout, compiler):
    outputs = set()
    for line in stdout.splitlines():
        if str(compiler) not in line:
            continue
        # Match the command, rather than the same text quoted in fixdep's
        # command receipt. A compile has both a -c and its own -o operand.
        words = shlex.shlex(line, posix=True, punctuation_chars=True)
        words.whitespace_split = True
        words.commenters = ""
        command = []
        for word in [*words, ";"]:
            if word and all(ch in "();<>|&" for ch in word):
                if command and command[0] == str(compiler) and "-c" in command and "-o" in command:
                    outputs.add(Path(command[command.index("-o") + 1]))
                command.clear()
            else:
                command.append(word)
    return outputs


def dry_run(source, tools, pins):
    reference = runpy.run_path(str(ROOT / "scripts/toolchain-busybox-c-inventory.py"))
    clang, reference_pins = reference["verify_inputs"]()
    if pins != reference_pins:
        raise ValueError("Kbuild and the Mac Clang build have different input pins")
    commands = reference["compile_commands"](clang)  # Also checks the target object receipts.
    target_objects = {
        Path(name) for name in (reference["WORK"] / "objects.txt").read_text().splitlines()
    }
    host_objects = {
        Path(command.output) for command in commands
        if command.kind == "host" and Path(command.output).suffix == ".o"
    }
    # A recursive dry run cannot produce headers for another sub-make.
    # Stage those inputs from the verified build, leaving its objects behind.
    for name in (
        ".config", ".kconfig.d", "include/autoconf.h", "include/applet_tables.h",
        "include/usage_compressed.h",
    ):
        shutil.copyfile(reference["SRC"] / name, source / name)

    args = ["-n", "-f", "Makefile"]
    bindings = [f"{key}={tools / name}" for key, name in TOOL_BINDINGS.items()]
    report_args = [
        "<tools>/bmake", *args,
        *(f"{key}=<tools>/{name}" for key, name in TOOL_BINDINGS.items()), "ARCH=x86_64",
    ]
    printed = set()
    for goal in ("allnoconfig", "applets/built-in.o"):
        result = subprocess.run(
            [str(tools / "bmake"), *args, *bindings, "ARCH=x86_64", goal], cwd=source,
            env=ENV, capture_output=True, text=True, timeout=60,
        )
        if result.returncode:
            raise ValueError(
                f"bmake -n {goal} exited {result.returncode}\n{result.stderr.rstrip()}"
            )
        printed.update(compile_outputs(result.stdout, tools / "bcc"))
    expected = target_objects | host_objects
    missing = expected - printed
    if missing:
        raise ValueError("bmake -n omitted Clang objects: " + ", ".join(map(str, sorted(missing))))
    return {
        "argv": [*report_args, "applets/built-in.o"],
        "configure_argv": [*report_args, "allnoconfig"],
        "status": result.returncode,
        "first_failure": None,
        "clang_objects": len(expected),
        "clang_host_objects": len(host_objects),
        "clang_target_objects": len(target_objects),
        "clang_objects_with_printed_recipe": len(expected & printed),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tools-dir", type=Path, default=ROOT / "toolchain/target/debug",
                        help="Cargo output directory for host bmake, bcc, bld, bar and bnm")
    parser.add_argument("--check", action="store_true", help="compare with the checked-in inventory")
    args = parser.parse_args()
    tools = args.tools_dir.resolve()
    try:
        for name in sorted({"bmake", *TOOL_BINDINGS.values()}):
            path = tools / name
            if not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError(f"missing executable host tool: {path}")
        with tempfile.TemporaryDirectory(prefix="breenix-kbuild-inventory-") as temp:
            source, pins = prepare_workload(Path(temp))
            files = makefiles(source)
            chapters = inventory(source, files, tools / "bmake")
            unsupported = [
                f"{item['kind']} {item['construct']}"
                for constructs in chapters.values() for item in constructs if not item["supported"]
            ]
            if unsupported:
                raise ValueError("unsupported Kbuild constructs: " + ", ".join(unsupported))
            report = {
                "workload": WORKLOAD,
                "manual": "GNU make 4.4.1, edition 0.77",
                "scope": [
                    "Source spellings from the patched tree and generated directory Kbuild files.",
                    "The scan visits both conditional arms and records each construct once per physical line.",
                    "Recipe recognition uses the default tab prefix and the inline semicolon form.",
                    "Constructs created by expansion, including eval, are outside the source inventory.",
                    "The dry run stages the verified Mac Clang build's configuration and generated headers, "
                    "then checks the allnoconfig and applets/built-in.o goals for each recorded object recipe.",
                ],
                "input_pins": INPUT_PINS.relative_to(ROOT).as_posix(),
                "inputs": pins,
                "files": files,
                "dry_run": dry_run(source, tools, pins),
                "chapters": chapters,
            }
        result = json.dumps(report, indent=2) + "\n"
        if args.check:
            if OUTPUT.read_text() != result:
                raise ValueError(f"inventory differs from {OUTPUT.relative_to(ROOT)}")
        else:
            with tempfile.TemporaryDirectory(dir=OUTPUT.parent) as temp:
                draft = Path(temp) / OUTPUT.name
                draft.write_text(result)
                draft.replace(OUTPUT)
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"{error}\n{error.stderr or error.stdout or ''}")
    except (OSError, ValueError, subprocess.TimeoutExpired, tarfile.TarError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
