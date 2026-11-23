#!/usr/bin/env python3
"""Check libc-suite.mk with Cargo-built host tools and a Breenix C-ABI libc."""

import argparse
import importlib.util
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[3]
SUITE = ROOT / "userspace/libc-suite"
MAKEFILE = ROOT / "toolchain/corpus/libc-suite.mk"
INCLUDE = ROOT / "libs/libbreenix-libc/include"
SUPPORT = ROOT / "toolchain/include"
LINKER_SCRIPT = ROOT / "userspace/programs/linker.ld"


def run(*args, cwd=None, check=True):
    with subprocess.Popen(
        args, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        env=dict(os.environ, LC_ALL="C"), start_new_session=True,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=max(0, DEADLINE - time.monotonic()))
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise AssertionError(f"timed out: {shlex.join(os.fsdecode(arg) for arg in args)}") from None
    result = subprocess.CompletedProcess(args, child.returncode, stdout, stderr)
    if check:
        assert result.returncode == 0, f"{shlex.join(os.fsdecode(arg) for arg in args)}\n{stdout}{stderr}"
    return result


def make(config, suite, category, build, libc, functions, check=True):
    return run(
        config.tools / "bmake", "-f", MAKEFILE,
        f"SUITE={suite}", f"CATEGORY={category}", f"BUILD={build}",
        f"LIBC={libc}", f"BRT={config.brt}", f"FUNCTIONS={functions}",
        f"INCLUDE={INCLUDE}", f"CC_INCLUDE={SUPPORT}", f"LINKER_SCRIPT={LINKER_SCRIPT}",
        *(f"{var}={config.tools / name}" for var, name in
          (("CC", "bcc"), ("AS", "bas"), ("LD", "bld"), ("AR", "bar"), ("NM", "bnm"))),
        check=check,
    )


def fixture(config, work, source, functions, mode="", members=None):
    suite = work / "userspace/libc-suite"
    suite.mkdir(parents=True)
    (suite / "math.c").write_text(source)
    (suite / "case.h").touch()
    (suite / "abi_check.c").write_text('_Static_assert(sizeof(long) == 8, "LP64");\n')
    (suite / "harness.c").write_text("void libc_case_missing(const char *name) { (void)name; }\n")
    (suite / "categories").write_text(f"math{mode}\n")
    declarations = work / "functions.txt"
    declarations.write_text("".join(f"{name}\n" for name in sorted(functions)))
    libc = work / "libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a"
    libc.parent.mkdir(parents=True)
    start = ".global _start\n_start:\n    call main\n    ret\n"
    if members is None:
        members = {"breenix": start}
    objects = []
    for member, assembly in members.items():
        src = work / f"{member}.s"
        obj = src.with_suffix(".o")
        src.write_text(".section .text\n" + assembly)
        run(os.environ["BREENIX_CC_CLANG"], "--target=x86_64-unknown-none", "-c", src, "-o", obj)
        objects.append(obj)
    run(config.tools / "bar", "rcs", libc, *objects)
    return suite, libc, declarations


def build_sh(config, work, suite):
    """Run the original build.sh with pinned ELF tools in an isolated tree."""
    tools = work / "scripts"
    tools.mkdir(exist_ok=True)
    if not (tools / "as").exists():
        (tools / "as").symlink_to(config.tools / "bas")
    if not (tools / "llvm-nm").exists():
        (tools / "llvm-nm").symlink_to(config.llvm_nm)
    wrapper = tools / "breenix-cc"
    wrapper.write_text(
        "#!/bin/sh\nshift 2\n"
        f'if [ "$1" = --tool-dir ]; then printf "%s\\n" {shlex.quote(str(tools))}; exit; fi\n'
        'for arg do\n  if [ "$arg" = -c ]; then\n'
        f'    exec {shlex.quote(str(config.tools / "bcc"))} -B {shlex.quote(str(tools))} "$@"\n'
        '  fi\ndone\n'
        f'exec {shlex.quote(str(config.tools / "bld"))} -T {shlex.quote(str(LINKER_SCRIPT))}'
        ' --gc-sections "$@" '
        + shlex.quote(str(work / "libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a"))
        + "\n"
    )
    wrapper.chmod(0o755)
    script = suite / "build.sh"
    shutil.copyfile(SUITE / "build.sh", script)
    return run("/bin/bash", script, "--arch", "x86_64", check=False)


def missing_functions_match_build_sh(config, work):
    suite, libc, declarations = fixture(
        config, work,
        "void missing_a(void); void missing_z(void); void available(void) {}\n"
        "int main(void) { missing_z(); available(); missing_a(); return 0; }\n",
        ["missing_a", "missing_z"],
    )
    reference = build_sh(config, work, suite)
    assert reference.returncode == 0, reference.stdout + reference.stderr
    out = work / "make"
    make(config, suite, "math", out, libc, declarations)
    recipe_inventory = suite / "build/x86_64/math-missing.txt"
    names = recipe_inventory.read_text().splitlines()
    assert names == ["missing_a", "missing_z"], names
    assert (out / "math-missing.txt").read_bytes() == recipe_inventory.read_bytes()
    shims = (out / "missing-math.c").read_text()
    assert re.findall(r'libc_case_missing\("([^"]+)"\);', shims) == names
    assert (out / "libc-math_test.elf").is_file()

    (suite / "categories").write_text("math complete\n")
    reference = build_sh(config, work, suite)
    rejected = make(config, suite, "math", work / "complete", libc, declarations, check=False)
    message = "math is complete but libc.a lacks: missing_a missing_z"
    for result in (reference, rejected):
        assert result.returncode != 0 and message in result.stderr, result.stdout + result.stderr
    assert not (work / "complete/libc-math_test.elf").exists()


def missing_data_is_hard_error(config, work):
    suite, libc, declarations = fixture(
        config, work,
        "extern void *stdin;\nint main(void) { return stdin != 0; }\n",
        [],
    )
    out = work / "make"
    result = make(config, suite, "math", out, libc, declarations, check=False)
    assert result.returncode != 0, result.stdout
    assert "math uses data Breenix's C library does not define: stdin" in result.stderr, result.stderr
    assert not (out / "libc-math_test.elf").exists()
    assert not (out / "missing-math.c").exists()


def archive_functions(tool, archive, declarations, llvm=False):
    records = set()
    for line in run(tool, "-A", "-g", archive).stdout.splitlines():
        prefix, _, fields = line.rpartition(": ")
        rest = fields.split()
        if len(rest) != 3 or rest[-1] not in declarations:
            continue
        _, kind, name = rest
        member = prefix.rsplit(":", 1)[1] if llvm else prefix.removeprefix(f"{archive}[").removesuffix("]")
        records.add((name, kind in "WV", member))
    return records


def bnm_audit_matches_llvm_nm(config, _work):
    declarations = public_functions()
    actual = archive_functions(config.tools / "bnm", config.libc, declarations)
    expected = archive_functions(config.llvm_nm, config.libc, declarations, llvm=True)
    assert expected and actual == expected, (expected - actual, actual - expected)
    for name in ("memcmp", "memcpy", "memmove", "memset", "strlen"):
        assert any(sym == name and weak and member.startswith("compiler_builtins")
                   for sym, weak, member in actual), name
        assert any(sym == name and not weak and not member.startswith("compiler_builtins")
                   for sym, weak, member in actual), name


def public_functions():
    # Discover declarations on the host. The guest recipe reads the staged
    # inventory as a plain text input.
    script = ROOT / "scripts/libc-symbol-audit.py"
    spec = importlib.util.spec_from_file_location("libc_symbol_audit", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    names = module.declared_functions("x86_64", module.public_headers())
    assert names
    return names


def linked_functions_come_from_breenix(config, work):
    strong = ".global selected\nselected:\n    ret\n"
    weak = ".weak selected\nselected:\n    ret\n"
    start = ".global _start\n_start:\n    call main\n    ret\n"
    source = "void selected(void); int main(void) { selected(); return 0; }\n"

    for label, members in (
        ("selected-weak", {"compiler_builtins-control": weak, "breenix": start, "own": strong}),
        ("archive-weak", {"compiler_builtins-control": weak, "breenix": start}),
    ):
        case = work / label
        suite, libc, declarations = fixture(config, case, source, ["selected"], members=members)
        out = case / "make"
        result = make(config, suite, "math", out, libc, declarations, check=False)
        assert result.returncode != 0, result.stdout
        if label == "selected-weak":
            assert "selected: linked definition is not from Breenix libc" in result.stderr, result.stderr
            assert re.search(r"^selected\tweak\t.*\(compiler_builtins-control\.o\)$",
                             (out / "math.map").read_text(), re.M)
        else:
            assert "selected: no strong Breenix libc definition" in result.stderr, result.stderr
        assert not (out / "libc-math_test.elf").exists()

    names = public_functions()
    declarations = work / "functions.txt"
    declarations.write_text("".join(f"{name}\n" for name in sorted(names)))
    categories = [line.split()[0] for line in (SUITE / "categories").read_text().splitlines()
                  if line.strip() and not line.lstrip().startswith("#")]
    assert categories == ["dirent-glob", "getopt-env", "locale-wchar", "math",
                          "pthread", "stdio", "string-stdlib"], categories
    out = work / "real"
    for category in categories:
        make(config, SUITE, category, out, config.libc, declarations)
        run(config.tools / "elfcheck", "--exec", out / f"libc-{category}_test.elf")
        assert (out / f"{category}.map").stat().st_size
        assert (out / f"{category}-missing.txt").read_text() == "", category
        # Category-local symbol work should scale with its own objects. The
        # archive inventory and link map are evidence and have separate checks.
        object_rows = (out / f"{category}.symbols").read_bytes().count(b"\n")
        for path in out.glob(f"{category}.*"):
            if path.suffix in {".o", ".s", ".map", ".brt"}:
                continue
            rows = path.read_bytes().count(b"\n")
            assert rows <= object_rows, f"{path.name}: {rows} rows for {object_rows} object symbols"
    run(config.tools / "elfcheck", out / "abi_check.o", out / "harness.o")


if __name__ == "__main__":
    checks = (missing_functions_match_build_sh, bnm_audit_matches_llvm_nm,
              missing_data_is_hard_error, linked_functions_come_from_breenix)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tools", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--libc", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--brt", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--llvm-nm", required=True, type=lambda value: Path(value).resolve())
    parser.add_argument("--check", choices=[check.__name__ for check in checks])
    config = parser.parse_args()
    os.environ.setdefault("BREENIX_CC_CLANG", str(config.llvm_nm.with_name("clang")))
    sys.dont_write_bytecode = True
    failed = False
    for check in checks:
        if config.check and config.check != check.__name__:
            continue
        with tempfile.TemporaryDirectory(prefix="libc-suite-mk-") as directory:
            DEADLINE = time.monotonic() + 55
            try:
                check(config, Path(directory))
            except AssertionError as error:
                print(f"FAIL libc_suite_mk::{check.__name__}\n{error}", file=sys.stderr)
                failed = True
            else:
                print(f"PASS libc_suite_mk::{check.__name__}")
    sys.exit(failed)
