#!/usr/bin/env python3
"""Build guest tools and prepare the x86 assembler, linker, C and make fixtures."""

from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
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
        *remap, "-L", f"native={libc}", "-C", f"link-arg=-T{ROOT / 'userspace/programs/linker.ld'}",
        "-C", "link-arg=--allow-multiple-definition", "-C", "default-linker-libraries=no",
        "-C", f"metadata=breenix_std_{fork}",
    ]
    guest_env = dict(env, __CARGO_TESTS_ONLY_SRC_ROOT=str(library),
                     CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags))
    packages = (*TOOLS, "bcc", *(('bmake',) if arch == 'x86_64' else ()))
    binaries = cargo_tools(cargo, guest_env, target_json, ROOT / "target/toolchain-bundle/guest", packages,
                          ("-Z", "build-std=std,panic_abort", "-Z", "build-std-features=compiler-builtins-mem"))
    dev_bin = dev / "bin"
    dev_bin.mkdir()
    tools = {}
    for name in (packages if arch == "x86_64" else TOOLS):
        executable = dev_bin / {"bcc": "cc", "bmake": "make"}.get(name, name)
        shutil.copy2(binaries / name, executable)
        tools[name] = {
            "cargoPackage": name, "target": target, "rustForkCommit": fork,
            "rustc": rustc_record, "sha256": digest(executable),
        }
        if name in ("bcc", "bmake"):
            tools[name]["path"] = f"bin/{executable.name}"
    return tools


def build(arch, work, dev, corpus):
    work.mkdir()
    clang, _ = reference.pinned_tool(
        Path(os.environ["BREENIX_TOOLCHAIN_CLANG"]),
        os.environ["BREENIX_TOOLCHAIN_CLANG_SHA256"], work,
    )
    cargo, env, host, rustc_record = cargo_environment()
    tools = build_guest_tools(arch, work, dev, clang, cargo, env, rustc_record)

    if arch == "x86_64":
        for tool, alias in (("bas", "as"), ("bld", "ld")):
            shutil.copy2(dev / "bin" / tool, dev / "bin" / alias)
        shutil.copytree(WORKSPACE / "corpus/c-core", corpus / "c-core")
        shutil.copytree(WORKSPACE / "corpus/make", corpus / "make")
        shutil.copytree(WORKSPACE / "corpus/project", corpus / "project")
        lld, _ = reference.pinned_tool(
            Path(os.environ["BREENIX_TOOLCHAIN_LLD"]),
            os.environ["BREENIX_TOOLCHAIN_LLD_SHA256"], work,
        )
        link = [*lld, "--fatal-warnings", "-static", "--gc-sections", "-T", str(dev / "linker.ld")]
        # Use relative object names so the lld map names no build directory.
        host_env = dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(remap_flags(env)))
        host_bins = cargo_tools(cargo, host_env, host, ROOT / "target/toolchain-bundle/host", ("bas", "bld", "bcc"))
        host_dev = work / "host-dev"
        shutil.copytree(dev, host_dev)
        for package, name in (("bas", "as"), ("bld", "ld"), ("bcc", "cc")):
            shutil.copy2(host_bins / package, host_dev / "bin" / name)
        candidates = corpus / "c-core/mac"
        candidates.mkdir(parents=True)
        deferred = {"aggregate-copy", "bitfield-values", "varargs", "alignment", "atomics",
                    "compound-literal", "designated-init", "generic"}
        names = (ROOT / "toolchain/corpus/cases").read_text().splitlines()
        if len(names) != 30 or not deferred.issubset(names):
            raise ValueError("unexpected C corpus")
        for name in names:
            if name not in deferred:
                reference.run([str(host_dev / "bin/cc"), "-o", str(candidates / (name + ".elf")),
                               str(corpus / (name + ".c"))], work)
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
        host_env = dict(env, CARGO_ENCODED_RUSTFLAGS="\x1f".join(remap_flags(env)))
        host_bas = cargo_tools(cargo, host_env, host, ROOT / "target/toolchain-bundle/host", ("bas",)) / "bas"
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

    (work / "tools.json").write_text(json.dumps(tools) + "\n", encoding="utf-8")


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
