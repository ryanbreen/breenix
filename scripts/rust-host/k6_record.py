#!/usr/bin/env python3
"""Capture the K6 Mac twin's GNU-flavor link lines and replay them via rust-lld."""

import argparse
from dataclasses import dataclass
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import subprocess
import sys
import tomllib

from rustc_port import CACHE, HOSTED, K5, MAC, ROOT, TOOL, guest_environment, sha256, write_json

K6 = K5.with_name("k6")
BUILD = ROOT / "build/rustc-port/k6"
WORKSPACE = ROOT / "toolchain"
PACKAGES = ("bas", "bld", "bar", "bnm", "bcc", "bmake")
PROBES = {"panic-abort": TOOL, "catch-unwind": HOSTED}
LIB = {TOOL: "lib/tools", HOSTED: "lib/hosted"}
SCRIPT = {TOOL: "linker.ld", HOSTED: "linker-hosted.ld"}
METADATA = "lib.rmeta"

# GNU ld 2.45 §2.1. Keep operands as separate argv entries where rustc did.
SWITCHES = {
    "-Bstatic": "static-libraries",
    "-Bdynamic": "shared-libraries",
    "--as-needed": "as-needed",
    "--eh-frame-hdr": "eh-frame-header",
    "--gc-sections": "section-gc",
    "--allow-multiple-definition": "multiple-definitions",
    "--strip-debug": "strip-debug",
}
OPERANDS = {"-L": "search", "-l": "library", "-T": "script", "-o": "output",
            "-z": "elf-option", "-flavor": "driver-flavor"}


@dataclass(frozen=True)
class Argument:
    kind: str
    tokens: tuple[str, ...]
    value: str | None = None

    def with_path(self, path):
        if self.kind == "file":
            return [path]
        if len(self.tokens) == 2:
            return [self.tokens[0], path]
        return [self.tokens[0].removesuffix(self.value) + path]


def classify(argv):
    args = iter(argv)
    for token in args:
        if token in SWITCHES:
            yield Argument(SWITCHES[token], (token,))
        elif token in OPERANDS:
            value = next(args, "")
            if not value or value.startswith("-"):
                raise ValueError(f"{token}: missing operand")
            yield Argument(OPERANDS[token], (token, value), value)
        elif token.startswith(("-L", "-l", "-T")) and len(token) > 2:
            yield Argument(OPERANDS[token[:2]], (token,), token[2:])
        elif token.startswith("-O") and token[2:].isdigit():
            yield Argument("linker-optimization", (token,))
        elif not token.startswith(("-", "@")) and Path(token).suffix in {".o", ".a", ".rlib"}:
            yield Argument("file", (token,), token)
        else:
            raise ValueError(f"unclassified rustc linker argument {token!r}")


def fixture_path(fixture, value):
    relative = PurePosixPath(value)
    if not relative.parts or relative.is_absolute() or ".." in relative.parts:
        raise ValueError(f"expected a fixture-relative path: {value}")
    return fixture / relative


def library_files(args, search_dirs):
    """Resolve -l using the recorded GNU search paths and -B state."""
    static = False
    for arg in args:
        if arg.kind in {"static-libraries", "shared-libraries"}:
            static = arg.kind == "static-libraries"
        if arg.kind != "library":
            continue
        name = arg.value
        if name.startswith(":"):
            candidates = [name[1:]]
        else:
            candidates = [f"lib{name}.a"] if static else [f"lib{name}.so", f"lib{name}.a"]
        if any(not item or item != Path(item).name for item in candidates):
            raise ValueError(f"invalid library name {name}")
        for index, directory in enumerate(search_dirs):
            match = next((directory / item for item in candidates if (directory / item).is_file()), None)
            if match is not None:
                yield index, match
                break
        else:
            raise ValueError(f"cannot resolve -l{name} in the linker's -L directories")


def link_inputs(argv, fixture):
    args = list(classify(argv))
    dirs = [fixture_path(fixture, arg.value) for arg in args if arg.kind == "search"]
    for arg in args:
        if arg.kind in {"file", "script", "output"}:
            fixture_path(fixture, arg.value)
    return {arg.value for arg in args if arg.kind in {"file", "script"}} | {
        path.relative_to(fixture).as_posix() for _, path in library_files(args, dirs)
    }


def capture_link(fixture, argv):
    args = list(classify(argv))
    output, = [Path(arg.value) for arg in args if arg.kind == "output"]
    name, = [item for item in (*PACKAGES, *PROBES)
             if output.name == item or output.name.startswith(f"{item}-")]
    directory = fixture / "links" / name
    directory.mkdir(parents=True)

    def sealed(path):
        return path.is_relative_to(fixture / "sysroot") or path.is_relative_to(fixture / "lib")

    def stage(source, destination):
        if source != destination:
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        return destination.relative_to(fixture).as_posix()

    search_dirs = [Path(arg.value).resolve() for arg in args if arg.kind == "search"]
    staged_dirs = [path if sealed(path) else directory / "search" / str(index)
                   for index, path in enumerate(search_dirs)]
    for path in staged_dirs:
        path.mkdir(parents=True, exist_ok=True)
    for index, source in library_files(args, search_dirs):
        stage(source, staged_dirs[index] / source.name)

    search = iter(staged_dirs)
    normalized = []
    reference = directory / "reference.elf"
    for index, arg in enumerate(args):
        if arg.kind in {"file", "script"}:
            source = Path(arg.value).resolve()
            destination = source if sealed(source) else directory / "inputs" / f"{index}-{source.name}"
            normalized.extend(arg.with_path(stage(source, destination)))
        elif arg.kind == "search":
            normalized.extend(arg.with_path(next(search).relative_to(fixture).as_posix()))
        elif arg.kind == "output":
            normalized.extend(arg.with_path(reference.relative_to(fixture).as_posix()))
        else:
            normalized.extend(arg.tokens)

    # Replay the saved line so the reference also verifies path normalization.
    rust_lld = json.loads((fixture / "capture.json").read_text())["rust_lld"]
    flavor = [] if normalized[:2] == ["-flavor", "gnu"] else ["-flavor", "gnu"]
    run([rust_lld, *flavor, *normalized], cwd=fixture, env={}, timeout=30)
    shutil.copy2(reference, output)
    write_json(directory / "argv.json", normalized)


def run(argv, **kwargs):
    result = subprocess.run(argv, check=True, text=True, capture_output=True, **kwargs)
    if re.search(r"(?im)^(?:[^\n]*?: )?warning:", result.stderr):
        raise ValueError(result.stderr)
    return result.stdout.strip()


def pinned_tools():
    channel = tomllib.loads((WORKSPACE / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    cargo = Path(run(["rustup", "which", "--toolchain", channel, "cargo"]))
    rustc = Path(run(["rustup", "which", "--toolchain", channel, "rustc"]))
    llvm = rustc.parent.parent / "lib/rustlib" / MAC / "bin"
    return channel, cargo, llvm


def llvm_tools():
    return pinned_tools()[2]


def inspect_input(path, tools, arch="x86_64"):
    kinds = []
    metadata = []
    if path.suffix != ".ld":
        relocations = run([tools / "llvm-readobj", "--elf-output-style=GNU", "-Wr", path], timeout=15)
        kinds = sorted(set(re.findall(r"^\s*[0-9a-fA-F]{16}\s+[0-9a-fA-F]{16}\s+(\S+)",
                                      relocations, re.M)))
        if any(not kind.startswith("R_X86_64_" if arch == "x86_64" else "R_AARCH64_") for kind in kinds):
            raise ValueError(f"{path}: unexpected relocation type in {kinds}")
    if path.suffix == ".rlib":
        metadata = [name for name in run([tools / "llvm-ar", "t", path], timeout=15).splitlines()
                    if name == METADATA]
    return {"sha256": sha256(path), "relocations": kinds, "metadata_members": metadata}


def reference_headers(path, tools, arch="x86_64"):
    headers = run([tools / "llvm-readobj", "--file-headers", "--program-headers", path], timeout=15)
    if not all(field in headers for field in ("Type: Executable", "Machine: EM_X86_64" if arch == "x86_64" else "Machine: EM_AARCH64")):
        raise ValueError(f"{path}: expected an {arch} executable")
    if "Type: PT_INTERP" in headers or "Type: PT_DYNAMIC" in headers:
        raise ValueError(f"{path}: expected a static executable")
    return "Type: PT_GNU_EH_FRAME" in headers


def bld_relocations(arch="x86_64"):
    source = (WORKSPACE / "bld/src/relocate.rs").read_text()
    if arch == "aarch64":
        dispatch = source.split("fn apply_aarch64_relocation(", 1)[1].split('let offset = field_offset', 1)[0]
        return set(re.findall(r"\bR_AARCH64_[A-Z0-9_]+\b", dispatch)) - {"R_AARCH64_NONE_LEGACY"}
    field_width = source.split("fn amd64_field_width(", 1)[1].split("\n}", 1)[0]
    return {f"R_X86_64_{kind}" for kind in re.findall(r"\bR_([A-Z0-9_]+)\b", field_width)}


def prepare(args, root, tools):
    root.mkdir(parents=True)
    for directory in ("bin", "targets", "cargo-home", *LIB.values()):
        (root / directory).mkdir(parents=True)
    for target, libc in ((TOOL, args.tools_libc), (HOSTED, args.hosted_libc)):
        shutil.copyfile(ROOT / f"{target}.json", root / "targets" / f"{target}.json")
        shutil.copyfile(libc, root / LIB[target] / "libc.a")
        shutil.copyfile(ROOT / "userspace/programs" / SCRIPT[target], root / LIB[target] / SCRIPT[target])
    shutil.copyfile(args.bunwind, root / LIB[HOSTED] / "libbunwind.a")
    shutil.copytree(K6 / "corpus", root / "corpus")

    workspace = root / "src/toolchain"
    shutil.copytree(WORKSPACE, workspace, ignore=shutil.ignore_patterns("target", "rust-host"))
    k5 = json.loads((K5 / f"{args.arch}.json").read_text())
    if sha256(args.rustc) != k5["mac_rustc_sha256"]:
        raise ValueError("rustc is not the recorded K5 Mac twin")
    for name, digest in k5["sysroot"]["files_sha256"].items():
        dest = root / "sysroot" / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(args.sysroot / name, dest)
        if sha256(dest) != digest:
            raise ValueError(f"K5 sysroot digest mismatch: {name}")

    # Use an absolute interpreter so the linker works in the cleared environment.
    # Configuration travels by the fixture path rather than extra environment keys.
    linker = root / "bin/k6-linker"
    command = shlex.join([sys.executable, str(Path(__file__).resolve()), "--link", str(root)])
    linker.write_text('#!/bin/sh\nexec ' + command + ' "$@"\n')
    linker.chmod(0o755)
    (root / "capture.json").write_text(json.dumps({"rust_lld": str(tools / "rust-lld")}))
    return k5


def rustflags(root, target):
    lib = root / LIB[target]
    return [
        f"--sysroot={root / 'sysroot'}", f"-Clinker={root / 'bin/k6-linker'}",
        f"-Clink-arg=-T{lib / SCRIPT[target]}", f"-Lnative={lib}",
        "-Clink-arg=--allow-multiple-definition", "-Cdefault-linker-libraries=no",
        f"--remap-path-prefix={root / 'src'}=/breenix-src",
    ]


def capture(args):
    channel, cargo, tools = pinned_tools()
    root = args.fixture.resolve()
    k5 = prepare(args, root, tools)
    env = guest_environment(root / "targets")
    rustc = args.rustc.resolve()
    flags = rustflags(root, TOOL)
    cargo_env = dict(env, RUSTC=str(rustc), CARGO_HOME=str(root / "cargo-home"),
                     CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags))
    # The explicit spec preserves the K5 target identity in the staged rlibs.
    cargo_args = [
        "build", "--offline", "--frozen", "--jobs", "4", "--profile", "selfhost",
        "--target", f"targets/{TOOL}.json", "--manifest-path", "src/toolchain/Cargo.toml",
        *(part for package in PACKAGES for part in ("-p", package)), "--target-dir", "target",
    ]
    run([cargo, *cargo_args], cwd=root, env=cargo_env, timeout=300)
    (root / "probes").mkdir()
    for name, target in PROBES.items():
        panic = "unwind" if target == HOSTED else "abort"
        run([rustc, "--edition=2021", "--target", root / "targets" / f"{target}.json",
             *rustflags(root, target), f"-Cpanic={panic}", "-Dwarnings",
             f"{name}.rs", "-o", root / "probes" / name],
            cwd=root / "corpus", env=env, timeout=30)

    links = {}
    inputs = {}
    bld = bld_relocations(args.arch)
    observed = set()
    for name in (*PACKAGES, *PROBES):
        directory = root / "links" / name
        argv = json.loads((directory / "argv.json").read_text())
        sources = sorted(link_inputs(argv, root))
        for path in sources:
            if path not in inputs:
                inputs[path] = inspect_input(root / path, tools, args.arch)
        kinds = {kind for path in sources for kind in inputs[path]["relocations"]}
        observed.update(kinds)
        reference = directory / "reference.elf"
        if reference_headers(reference, tools, args.arch) != (name == "catch-unwind"):
            raise ValueError(f"{name}: unexpected unwind header")
        links[name] = {
            "argv": argv,
            "argument_kinds": [arg.kind for arg in classify(argv)],
            "inputs": sources,
            "relocations": sorted(kinds),
            "needs_bld": sorted(kinds - bld),
            "reference_sha256": sha256(reference),
        }

    record = {
        "rustc_sha256": k5["mac_rustc_sha256"],
        "rustc_commit": k5["fork_commit"],
        "cargo_channel": channel,
        "cargo_version": run([cargo, "--version"], env={}, timeout=15),
        "rust_lld_sha256": sha256(tools / "rust-lld"),
        "cargo_argv": cargo_args,
        "cargo_environment": {
            "RUSTC": "<k5>/mac/bin/rustc",
            "RUST_TARGET_PATH": "targets",
            "CARGO_HOME": "cargo-home",
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flag.replace(str(root) + "/", "") for flag in flags),
        },
        "rlib_metadata_member": METADATA,
        "links": links,
        "inputs": dict(sorted(inputs.items())),
        "relocations": {
            "observed": sorted(observed),
            "supported_by_bld": sorted(observed & bld),
            "needs_bld": sorted(observed - bld),
        },
    }
    write_json(root / f"{args.arch}.json", record)
    print(f"Recorded {len(links)} links and {len(inputs)} inputs in {root}")


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "--link":
        capture_link(Path(sys.argv[2]), sys.argv[3:])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("x86_64", "aarch64"), default="x86_64")
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--rustc", type=Path, default=CACHE / "mac/bin/rustc")
    parser.add_argument("--sysroot", type=Path, default=CACHE / "sysroot")
    parser.add_argument("--tools-libc", type=Path, required=True)
    parser.add_argument("--hosted-libc", type=Path, required=True)
    parser.add_argument("--bunwind", type=Path, required=True)
    args = parser.parse_args()
    global TOOL, HOSTED, PROBES, LIB, SCRIPT
    if args.arch == "aarch64":
        TOOL, HOSTED = "aarch64-breenix", "aarch64-breenix-hosted"
        PROBES = {"panic-abort": TOOL, "catch-unwind": HOSTED}
        LIB = {TOOL: "lib/tools", HOSTED: "lib/hosted"}
        SCRIPT = {TOOL: "linker.ld", HOSTED: "linker-hosted.ld"}
    if args.fixture is None:
        args.fixture = BUILD / ("fixture" if args.arch == "x86_64" else "aarch64")
    capture(args)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.exit(error.stderr or str(error))
    except (OSError, ValueError) as error:
        sys.exit(str(error))
