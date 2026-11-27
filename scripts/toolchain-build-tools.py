#!/usr/bin/env python3
"""Build guest tools and prepare the assembler, linker, C and make fixtures."""

import argparse
from importlib import import_module
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

sys.dont_write_bytecode = True
loader = SourceFileLoader("toolchain_ref_cc", str(Path(__file__).with_name("toolchain-ref-cc")))
reference = module_from_spec(spec_from_loader(loader.name, loader))
loader.exec_module(reference)
ROOT = reference.ROOT
FIXTURES = ROOT / "toolchain/corpus/assembler"
WORKSPACE = ROOT / "toolchain"
TOOLS = ("bas", "bld", "bar", "bnm")
LINKER_SOURCES = ("got-main.c", "got-entry.s", "archive-value-with-long-name.s", "tls-template.c")
PROGRAMS = ("sse-exit", "x87-exit")


def digest(path):
    return reference.inventory.file_record(path)["sha256"]


def cargo_environment():
    channel = tomllib.loads((WORKSPACE / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    rustc = Path(subprocess.check_output(
        ["rustup", "which", "--toolchain", channel, "rustc"], text=True,
    ).strip())
    env = dict(os.environ, RUSTC=str(rustc))
    for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        env.pop(key, None)
    version = subprocess.check_output([str(rustc), "-vV"], text=True)
    host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
    rustc_record = {"version": version.splitlines()[0], "sha256": digest(rustc)}
    return ["cargo", f"+{channel}", "build", "--release", "--quiet"], env, host, rustc_record


def remap_flags(env):
    if env.get("BREENIX_USERSPACE_REMAP") != "1":
        return []
    cargo_home = Path(env.get("CARGO_HOME", Path.home() / ".cargo"))
    return [f"--remap-path-prefix={ROOT}=/breenix", f"--remap-path-prefix={cargo_home}=/cargo"]


def cargo_tools(cargo, env, target, target_dir, packages, extra=()):
    subprocess.run([
        *cargo, "--locked", "--manifest-path", str(WORKSPACE / "Cargo.toml"),
        "--target", str(target), "--target-dir", str(target_dir),
        *(flag for package in packages for flag in ("-p", package, "--bin", package)),
        *extra,
    ], cwd=WORKSPACE, env=env, check=True)
    return target_dir / Path(target).stem / "release"


def arm_guest_link_inputs(target_dir, archive, linker):
    # The bundle uses a temporary staging path. Keep Cargo's flags stable for
    # identical inputs and use separate outputs when either input changes.
    inputs = [(source, digest(source)) for source in (archive, linker)]
    target_dir = target_dir / "arm-link" / "-".join(sha256 for _, sha256 in inputs)
    target_dir.mkdir(parents=True, exist_ok=True)
    for source, sha256 in inputs:
        destination = target_dir / source.name
        if destination.is_file() and digest(destination) == sha256:
            continue
        with tempfile.TemporaryDirectory(dir=target_dir) as temp:
            staged = Path(temp) / source.name
            shutil.copyfile(source, staged)
            if digest(staged) != sha256:
                raise ValueError(f"link input changed while staging: {source}")
            staged.replace(destination)
    return target_dir


def build_guest_tools(arch, work, dev, clang, cargo, env, rustc_record):
    library = Path(env.get("BREENIX_RUST_FORK_LIBRARY", ROOT / "rust-fork/library")).resolve()
    subprocess.run([str(ROOT / "scripts/rust-fork.sh"), "verify"], env=env, check=True)
    fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
    snapshot = library.parent / ".rust-fork-snapshot.json"
    actual_fork = (json.loads(snapshot.read_text())["commit"] if snapshot.is_file() else
                   subprocess.check_output(["git", "-C", str(library.parent), "rev-parse", "HEAD"],
                                           text=True).strip())
    if actual_fork != fork:
        raise ValueError("the toolchain bundle requires the pinned Breenix Rust library")

    target = f"{arch}-breenix"
    target_json = ROOT / f"{target}.json"
    remap = remap_flags(env)
    libc = dev / "lib"
    linker = ROOT / "userspace/programs/linker.ld"
    target_dir = ROOT / "target/toolchain-bundle/guest"
    if arch == "aarch64":
        target_dir = arm_guest_link_inputs(target_dir, libc / "libc.a", linker)
        libc = target_dir
        linker = target_dir / linker.name
    if arch == "x86_64":
        # The C reference corpus uses the SysV float ABI. Rust tools use the
        # soft-float target and the matching libc.a, as userspace/build.sh does.
        libc_target = ROOT / "target/toolchain-bundle/rust-libc"
        subprocess.run([
            *cargo, "--target", str(target_json), "--target-dir", str(libc_target),
        ], cwd=ROOT / "libs/libbreenix-libc",
            env=dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(remap)), check=True)
        libc = libc_target / target / "release"
        subprocess.run([
            sys.executable, str(ROOT / "scripts/libc-clean-archive.py"),
            "--archive", str(libc / "libc.a"), "--tool-dir", str(Path(clang[0]).parent),
        ], check=True)

    flags = [
        *remap, "-L", f"native={libc}", "-C", f"link-arg=-T{linker}",
        "-C", "link-arg=--allow-multiple-definition", "-C", "default-linker-libraries=no",
        "-C", f"metadata=breenix_std_{fork}",
    ]
    guest_env = dict(env, __CARGO_TESTS_ONLY_SRC_ROOT=str(library),
                     CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags))
    packages = (*TOOLS, "bcc", "bmake")
    binaries = cargo_tools(cargo, guest_env, target_json, target_dir, packages,
                          ("-Z", "build-std=std,panic_abort", "-Z", "build-std-features=compiler-builtins-mem"))
    tools = {}
    # The interop fixture crosses the C ABI, including SSE float registers.
    interop_target = ROOT / "x86_64-breenix-c.json" if arch == "x86_64" else target_json
    brt_features = ("--features", "exports") if arch == "aarch64" else ()
    archives = (
        ("interop", interop_target, WORKSPACE / "corpus/interop/rust/Cargo.toml", ()),
        ("brt", target_json, WORKSPACE / "Cargo.toml", ("--locked", "-p", "brt", "--lib", *brt_features)),
    )
    archive_flags = [*remap, "-C", f"metadata=breenix_std_{fork}"]
    archive_env = dict(env, __CARGO_TESTS_ONLY_SRC_ROOT=str(library),
                       CARGO_ENCODED_RUSTFLAGS="\x1f".join(archive_flags))
    for package, archive_target, manifest, selection in archives:
        archive_dir = ROOT / "target/toolchain-bundle" / package
        subprocess.run([
            *cargo, "--manifest-path", str(manifest), *selection,
            "--target", str(archive_target), "--target-dir", str(archive_dir),
            "-Z", "build-std=core",
        ], cwd=WORKSPACE, env=archive_env, check=True)
        filename = f"lib{package}.a"
        destination = dev / "lib" / filename
        shutil.copy2(archive_dir / archive_target.stem / "release" / filename, destination)
        tools[package] = {
            "cargoPackage": package,
            "path": destination.relative_to(dev).as_posix(),
            "target": archive_target.stem,
            "rustForkCommit": fork,
            "rustc": rustc_record,
            "sha256": digest(destination),
        }
    dev_bin = dev / "bin"
    dev_bin.mkdir()
    for name in packages:
        executable = dev_bin / {"bcc": "cc", "bmake": "make"}.get(name, name)
        shutil.copy2(binaries / name, executable)
        tools[name] = {
            "cargoPackage": name, "target": target, "rustForkCommit": fork,
            "rustc": rustc_record, "sha256": digest(executable),
        }
        if name in ("bcc", "bmake"):
            tools[name]["path"] = f"bin/{executable.name}"
    return tools


def stage_kernel_assembly(image, arch, corpus, milestone, record):
    if milestone != image.KERNEL_ASSEMBLY_MILESTONE:
        if record is not None:
            raise ValueError(f"--kernel-assembly-record requires {image.KERNEL_ASSEMBLY_MILESTONE}")
        return
    if record is None:
        raise ValueError(f"{image.KERNEL_ASSEMBLY_MILESTONE} requires --kernel-assembly-record")
    fields = image.read_kernel_assembly_record(record, arch)
    if fields["bas_sha256"] != digest(record.parent / "bas"):
        raise ValueError("kernel assembly record differs from the build host's bas")
    destination = corpus / image.KERNEL_ASSEMBLY.name
    sources, units, objects, asm_corpus = image.KERNEL_ASM_FILES[arch]
    for names, directory in ((sources, ROOT / "kernel"),
                             ((*units, *objects), record.parent)):
        for name in names:
            staged = destination / name
            staged.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(directory / name, staged)
    shutil.copyfile(record, destination / image.KERNEL_ASSEMBLY_RECORD)
    for name in asm_corpus:
        shutil.copyfile(WORKSPACE / "corpus/kernel-asm" / name, destination / name)


def build(arch, work, dev, corpus, milestone=None, assembly_record=None):
    image = import_module("toolchain-image")
    stage_kernel_assembly(image, arch, corpus, milestone, assembly_record)
    work.mkdir()
    clang, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
        os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work,
    )
    cargo, env, host, rustc_record = cargo_environment()
    tools = build_guest_tools(arch, work, dev, clang, cargo, env, rustc_record)

    for tool, alias in image.DRIVER_TOOL_ALIASES:
        shutil.copy2(dev / "bin" / tool, dev / "bin" / alias)
    for category in ("c-core", "make", "project"):
        shutil.copytree(WORKSPACE / "corpus" / category, corpus / category)
    if arch == "aarch64":
        shutil.copy2(WORKSPACE / "corpus/project-aarch64/value.s", corpus / "project/value.s")
    host_env = dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(remap_flags(env)))
    host_bins = cargo_tools(cargo, host_env, host, ROOT / "target/toolchain-bundle/host", ("bas", "bld", "bcc"))
    host_dev = work / "host-dev"
    shutil.copytree(dev, host_dev)
    for package, name in (("bas", "as"), ("bld", "ld"), ("bcc", "cc")):
        shutil.copy2(host_bins / package, host_dev / "bin" / name)
    candidates = corpus / "c-core/mac"
    candidates.mkdir(parents=True)
    names = (ROOT / "toolchain/corpus/cases").read_text().splitlines()
    if len(names) != 30:
        raise ValueError("unexpected C corpus")
    for name in names:
        reference.run([str(host_dev / "bin/cc"), *(["--target=aarch64"] if arch == "aarch64" else []),
                           "-o", str(candidates / (name + ".elf")), str(corpus / (name + ".c"))], work)

    stage_bootstrap_abi_tls(arch, clang, work, corpus)
    if arch == "x86_64":
        lld, _ = reference.pinned_tool(
            Path(os.environ["BREENIX_TOOLCHAIN_LLD"]),
            os.environ["BREENIX_TOOLCHAIN_LLD_SHA256"], work,
        )
        link = [*lld, "--fatal-warnings", "-static", "--gc-sections", "-T", str(dev / "linker.ld")]
        # Use relative object names so the lld map names no build directory.

        host_bas = host_bins / "bas"
        producer = {"cargoPackage": "bas", "target": host, "sha256": digest(host_bas)}
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
                "hostBas": producer,
                "inputObjects": {obj: obj_hash},
                "elf": {"name": elf, "sha256": digest(work / elf)},
                "map": map_text,
            }
            (work / elf).rename(destination / elf)
            (destination / f"{name}.link.json").write_text(json.dumps(receipt, indent=2) + "\n",
                                                           encoding="utf-8")

        build_linker_objects(arch, clang, work, corpus)

    if arch == "aarch64":
        lld, _ = reference.pinned_tool(
            Path(os.environ["BREENIX_TOOLCHAIN_LLD"]),
            os.environ["BREENIX_TOOLCHAIN_LLD_SHA256"], work,
        )
        link = [*lld, "--fatal-warnings", "-static", "--gc-sections", "-T", str(dev / "linker.ld")]
        host_bas = host_bins / "bas"
        producer = {"cargoPackage": "bas", "target": host, "sha256": digest(host_bas)}
        destination = corpus / "assembler"
        destination.mkdir()
        fixtures = WORKSPACE / "corpus/assembler-aarch64"
        for name in ("encodings.s", "encodings.hex", "bad-input.s", "fp-exit.s", "fp-exit.expect",
                     "call-exit.s", "call-exit.expect"):
            shutil.copy2(fixtures / name, destination / name)
        for name in ("fp-exit", "call-exit"):
            obj, elf, link_map = (f"{name}.{ext}" for ext in ("o", "elf", "map"))
            reference.run([str(host_bas), "--target", "aarch64", "-o", obj, str(destination / f"{name}.s")], work)
            obj_hash = digest(work / obj)
            trace = reference.run([*link, "--trace", f"-Map={link_map}", "-o", elf, obj], work)
            map_text = (work / link_map).read_text(encoding="utf-8")
            if (trace.splitlines() != [obj] or digest(work / obj) != obj_hash or
                    not any(line.split()[-1:] == [f"{obj}:(.text)"] for line in map_text.splitlines())):
                raise ValueError(f"{elf}: lld trace or map differs from the expected input {obj}")
            receipt = {"hostBas": producer, "inputObjects": {obj: obj_hash},
                       "elf": {"name": elf, "sha256": digest(work / elf)}, "map": map_text}
            (work / elf).rename(destination / elf)
            (destination / f"{name}.link.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")

        build_linker_objects(arch, clang, work, corpus)

    if milestone in image.BUSYBOX_MILESTONES:
        for path, source in image.busybox_inputs().items():
            destination = corpus / path.relative_to(image.CORPUS)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)

    (work / "tools.json").write_text(json.dumps(tools) + "\n", encoding="utf-8")


def stage_bootstrap_abi_tls(arch, clang, work, corpus):
    for category in ("abi", "tls", "interop"):
        name = "abi-aarch64" if arch == "aarch64" and category == "abi" else category
        source_dir = WORKSPACE / "corpus" / name
        destination = corpus / name
        destination.mkdir()
        for source in sorted(source_dir.iterdir()):
            if not source.is_file() or source.suffix not in {".c", ".expect", ".h"}:
                continue
            staged = destination / source.name
            shutil.copy2(source, staged)
        if category == "abi":
            for staged in sorted(destination.glob("*.c")):
                compiled, _, _ = reference.compile_c(arch, clang, staged, (destination,), work)
                compiled.rename(staged.with_suffix(".o"))


def build_linker_objects(arch, clang, work, corpus):
    sources = ROOT / "toolchain/corpus/linker"
    destination = corpus / "linker"
    destination.mkdir()
    for name in ("got-main", "tls-template"):
        shutil.copyfile(sources / f"{name}.expect", destination / f"{name}.expect")
    for name in LINKER_SOURCES:
        source = (WORKSPACE / "corpus/linker-aarch64" / name
                  if arch == "aarch64" and name.endswith(".s") else sources / name)
        obj = corpus / "objs" / (source.stem + ".o")
        if source.suffix == ".c":
            compiled, _, _ = reference.compile_c(arch, clang, source, (sources,), work)
            compiled.rename(obj)
        else:
            reference.run([
                *clang, "--no-default-config", f"--target={arch}-unknown-none-elf",
                "-Werror", "-fintegrated-as", "-x", "assembler", "-c", str(source), "-o", str(obj),
            ], work)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("arch", choices=("x86_64", "aarch64"))
    parser.add_argument("work", type=Path)
    parser.add_argument("dev", type=Path)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--milestone")
    parser.add_argument("--kernel-assembly-record", type=Path)
    args = parser.parse_args()
    try:
        build(args.arch, args.work, args.dev, args.corpus, args.milestone, args.kernel_assembly_record)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"toolchain-build-tools: {error}")
