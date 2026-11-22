#!/usr/bin/env python3
"""Build guest tools and prepare the x86 assembler and linker fixtures."""

from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
loader = SourceFileLoader("toolchain_ref_cc", str(Path(__file__).with_name("toolchain-ref-cc")))
reference = module_from_spec(spec_from_loader(loader.name, loader))
loader.exec_module(reference)
ROOT = reference.ROOT
FIXTURES = ROOT / "toolchain/corpus/assembler"
TOOLS = {
    "bas": ("bas/bas.c", "common/elf64.c"),
    "bld": ("bld/bld.c", "bld/script.c", "common/ar.c"),
    "bar": ("bar/bar.c", "common/ar.c", "common/elf_symbols.c"),
    "bnm": ("bnm/bnm.c", "common/ar.c", "common/elf_symbols.c"),
}
LINKER_SOURCES = ("got-main.c", "got-entry.s", "archive-value-with-long-name.s", "tls-template.c")
PROGRAMS = ("sse-exit", "x87-exit")
STRICT = ("-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic")


def digest(path):
    return reference.inventory.file_record(path)["sha256"]


def build(arch, work, dev, corpus):
    work.mkdir()
    clang, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
        os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work,
    )
    lld, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_LLD"]),
        os.environ["BREENIX_TOOLCHAIN_LLD_SHA256"], work,
    )

    headers = (ROOT / "libs/libbreenix-libc/include", ROOT / "toolchain/include")
    dev_bin = dev / "bin"
    dev_bin.mkdir()
    link = [*lld, "--fatal-warnings", "-static", "--gc-sections", "-T", str(dev / "linker.ld")]
    built = {}
    for tool, sources in TOOLS.items():
        objects = []
        for relative in sources:
            if relative not in built:
                source = ROOT / "toolchain" / relative
                obj = relative.replace("/", "-") + ".o"
                reference.run([
                    *clang, "--no-default-config", f"--target={arch}-unknown-linux-gnu",
                    "-Ulinux", "-U__linux", "-U__linux__", "-U__gnu_linux__", "-D__breenix__=1",
                    *STRICT, "-nostdinc", "-isystem", str(headers[0]), "-isystem", str(headers[1]),
                    "-fno-stack-protector", "-fno-pic", "-fno-pie", reference.ARCH_FLAGS[arch],
                    "-c", str(source), "-o", obj,
                ], work)
                built[relative] = obj
            objects.append(built[relative])
        reference.run([*link, "-o", str(dev_bin / tool), *objects, str(dev / "lib/libc.a")], work)

    if arch == "x86_64":
        # Host bas emits the exact objects linked by lld. Use relative input
        # names in the map so the receipt names no build directory.
        host_bas = work / "host-bas"
        host_flags = ["--no-default-config", *STRICT]
        if sys.platform == "darwin":
            sdk = subprocess.check_output(
                ["/usr/bin/xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True,
            ).strip()
            host_flags.extend(["-isysroot", sdk])
        subprocess.run([*clang, *host_flags, *(str(ROOT / "toolchain" / source) for source in TOOLS["bas"]),
                        "-o", str(host_bas)], cwd=work, check=True)
        destination = corpus / "assembler"
        shutil.copytree(FIXTURES, destination)
        for name in PROGRAMS:
            obj, elf, link_map = (f"{name}.{ext}" for ext in ("o", "elf", "map"))
            reference.run([str(host_bas), "-o", obj, str(destination / f"{name}.s")], work)
            obj_hash = digest(work / obj)
            trace = reference.run([*link, "--trace", f"-Map={link_map}", "-o", elf, obj], work)
            map_text = (work / link_map).read_text(encoding="utf-8")
            if (trace.splitlines() != [obj] or digest(work / obj) != obj_hash or
                    not any(line.split()[-1:] == [f"{obj}:(.text)"] for line in map_text.splitlines())):
                raise ValueError(f"{elf}: lld trace or map differs from the expected input {obj}")
            receipt = {
                "inputObjects": {obj: obj_hash},
                "elf": {"name": elf, "sha256": digest(work / elf)},
                "map": map_text,
            }
            (work / elf).rename(destination / elf)
            (destination / f"{name}.link.json").write_text(json.dumps(receipt, indent=2) + "\n",
                                                           encoding="utf-8")

        build_linker_objects(arch, clang, work, corpus)


def build_linker_objects(arch, clang, work, corpus):
    sources = ROOT / "toolchain/corpus/linker"
    destination = corpus / "linker"
    destination.mkdir()
    for name in ("got-main", "tls-template"):
        shutil.copyfile(sources / f"{name}.expect", destination / f"{name}.expect")
    for name in LINKER_SOURCES:
        source = sources / name
        obj = corpus / "objs" / (source.stem + ".o")
        if source.suffix == ".c":
            compiled, _, _ = reference.compile_c(arch, clang, source, (sources,), work)
            compiled.rename(obj)
        else:
            reference.run([
                *clang, "--no-default-config", "--target=x86_64-unknown-none-elf",
                "-Werror", "-fintegrated-as", "-x", "assembler", "-c", str(source), "-o", str(obj),
            ], work)


if __name__ == "__main__":
    try:
        build(sys.argv[1], *map(Path, sys.argv[2:]))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"toolchain-build-tools: {error}")
