"""Compile the unchanged libc recipe inputs with bcc and link with bld."""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SUITE = ROOT / "userspace/libc-suite"
LIBC = ROOT / "libs/libbreenix-libc"
LINKER_SCRIPT = ROOT / "userspace/programs/linker.ld"
# The same data symbols build.sh refuses to stand in for.
DATA = set("stdin stdout stderr optind optarg opterr optopt signgam environ timezone daylight tzname".split())


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, timeout=10)


def require(result):
    assert result.returncode == 0 and not result.stderr, (
        f"{result.args[0]}: {result.returncode}\n{result.stdout}{result.stderr}"
    )
    return result.stdout


def symbols(nm, path):
    records = re.findall(r"^(\S+) ([A-Za-z?])(?: [0-9a-f]+ [0-9a-f]+)?$", require(run(nm, "-gP", path)), re.M)
    assert records, f"empty symbol inventory for {path}"
    return (
        {name for name, kind in records if kind not in ("U", "v", "w")},
        {name for name, kind in records if kind in ("U", "v", "w")},
    )


def compile_suite(bcc, work, target):
    # Match build.sh's C recipe at -O0, with the sealed support headers.
    flags = [
        f"--target={target}", "-std=c17", "-fno-builtin", "-frounding-math",
        "-O0", "-Wall", "-Werror", "-I", SUITE, "-I", LIBC / "include",
        "-I", ROOT / "toolchain/include",
    ]
    tools = work / "bin"
    if not tools.exists():
        tools.mkdir()
        (tools / "as").symlink_to(bcc.with_name("bas"))
    categories = [
        line.split() for line in (SUITE / "categories").read_text().splitlines()
        if line.strip() and not line.startswith("#")
    ]
    assert len(categories) == 7 and len({row[0] for row in categories}) == 7
    failures = []
    objects = work / target
    objects.mkdir()
    for name in ("abi-check", "harness", *(row[0] for row in categories)):
        source = SUITE / (name.replace("-", "_") + ".c")
        obj = objects / f"{name}.o"
        result = run(bcc, *flags, "-B", tools, "-c", source, "-o", obj)
        if target == "aarch64" and result.returncode == 1:
            # Ruling 37 placeholders require a located compiler diagnostic.
            if re.fullmatch(r".+:\d+:\d+: error: [^\n]*ARM 25\.10b[^\n]*\n", result.stderr):
                assert not obj.exists() and not result.stdout
                print(f"{name}: {result.stderr.strip()}")
                continue
        try:
            require(result)
            assert obj.is_file(), f"{name} produced no object"
        except AssertionError as error:
            failures.append(str(error))
    assert not failures, "\n".join(failures)
    return flags, tools, categories, objects


def link_suite(bcc, recipe):
    flags, tools, categories, objects = recipe
    libc = LIBC / "target/x86_64-breenix-c/release/libc.a"
    nm = bcc.with_name("bnm")
    elfcheck = bcc.with_name("elfcheck")
    libc_defined, _ = symbols(nm, libc)
    # bcc's normal link supplies brt before libc for compiler-generated
    # atomics, including pthread.c's load through an atomic pointer.
    brt = Path(os.environ.get(
        "BREENIX_LIBC_SUITE_BRT",
        bcc.parent.parent / "x86_64-breenix/release/libbrt.a",
    ))
    brt_defined, _ = symbols(nm, brt)
    atomic_helpers = {
        "__atomic_load", "__atomic_store", "__atomic_exchange", "__atomic_compare_exchange",
    }
    harness = objects / "harness.o"
    harness_defined, harness_undefined = symbols(nm, harness)
    require(run(elfcheck, objects / "abi-check.o", harness))
    for category, *mode in categories:
        assert mode in ([], ["complete"]), (category, mode)
        obj = objects / f"{category}.o"
        defined, undefined = symbols(nm, obj)
        if category == "string-stdlib":
            ordinary_calls = {"memset", "memcmp", "strlen", "malloc", "free"}
            assert ordinary_calls <= undefined & libc_defined, (
                f"-fno-builtin lost calls: {ordinary_calls - (undefined & libc_defined)}"
            )
        missing = (undefined | harness_undefined) - (defined | harness_defined | libc_defined)
        missing -= brt_defined & atomic_helpers
        assert not missing & DATA, f"{category} missing data: {missing & DATA}"
        assert not (mode and missing), f"{category} missing functions: {missing}"
        assert not any(name.startswith("__builtin_") for name in missing), missing
        inputs = [obj, harness]
        if missing:
            source = objects / f"missing-{category}.c"
            shim = source.with_suffix(".o")
            source.write_text(
                "void libc_case_missing(const char *name);\n"
                + "".join(f'void {name}(void) {{ libc_case_missing("{name}"); }}\n'
                          for name in sorted(missing))
            )
            require(run(bcc, *flags, "-B", tools, "-c", source, "-o", shim))
            inputs.append(shim)
        elf = objects / f"{category}.elf"
        require(run(bcc.with_name("bld"), "-T", LINKER_SCRIPT, "--gc-sections",
                    "-o", elf, *inputs, brt, libc))
        require(run(elfcheck, "--exec", elf))
        print(f"{category}: ELF valid, stand-ins {', '.join(sorted(missing)) or 'none'}")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="bcc-libc-suite-") as directory:
        bcc = Path(sys.argv[1]).resolve()
        work = Path(directory)
        if sys.argv[2] == "x86_64":
            link_suite(bcc, compile_suite(bcc, work, "x86_64"))
        else:
            compile_suite(bcc, work, "aarch64")
