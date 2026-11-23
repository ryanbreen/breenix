#!/usr/bin/env python3
"""Seal and stage the toolchain suite's files on a private ext2 work copy."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

sys.path.insert(0, str(Path(__file__).resolve().parent / "metal"))
from metal_root import require_e2fsprogs

ROOT = Path(__file__).resolve().parents[1]
DEV = Path("opt/breenix-dev/25.2")
CORPUS = Path("usr/share/breenix/toolchain")
SUITE = Path("sbin/suite-toolchain")
TOOLS = ("bas", "bld", "bar", "bnm")
TOOL_PATHS = {DEV / "bin" / tool for tool in (*TOOLS, "cc", "make", "as", "ld")}
LINKER_OBJECTS = ("got-main", "got-entry", "archive-value-with-long-name", "tls-template")
MANIFEST = DEV / "manifest.sha256"
RECORD = DEV / "build.json"
PREFIXES = (Path("opt/breenix-dev"), CORPUS)
SHA256 = re.compile(r"[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")
NAME = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
GUEST_PATH = re.compile(r"[A-Za-z0-9_./+-]+")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def check_corpus(corpus):
    names = (corpus / "cases").read_text().splitlines()
    if not names or names != sorted(set(names)) or any(not NAME.fullmatch(name) for name in names):
        raise ValueError("invalid corpus case list")
    for suffix in (".c", ".expect"):
        if {path.stem for path in corpus.glob("*" + suffix)} != set(names):
            raise ValueError(f"case list differs from corpus {suffix} files")
    for name in names:
        status, newline, _ = (corpus / (name + ".expect")).read_bytes().partition(b"\n")
        if not newline or not re.fullmatch(rb"(0|[1-9][0-9]{0,2})", status) or int(status) > 255:
            raise ValueError(f"invalid expected exit status for {name}")
    return names


def payload(tree):
    files = []
    for path in sorted(tree.rglob("*")):
        relative = path.relative_to(tree)
        if path.is_symlink() or not GUEST_PATH.fullmatch(relative.as_posix()):
            raise ValueError(f"invalid fixture path: {relative}")
        if path.is_dir():
            if not any(relative.is_relative_to(prefix) or relative in prefix.parents
                       for prefix in (DEV, CORPUS, SUITE)):
                raise ValueError(f"unexpected fixture directory: {relative}")
        elif path.is_file() and (relative == SUITE or
                                any(relative.is_relative_to(prefix) for prefix in (DEV, CORPUS))):
            files.append(relative)
        else:
            raise ValueError(f"unexpected fixture file: {relative}")
    if SUITE not in files or not all((tree / prefix).is_dir() for prefix in (DEV, CORPUS)):
        raise ValueError("the fixture needs both payload prefixes and /sbin/suite-toolchain")
    return files


def manifest_bytes(tree, files):
    return "".join(f"{digest(tree / path)}  {path.as_posix()}\n" for path in files).encode()


def seal(bundle, commit, arch, tool_record):
    if not COMMIT.fullmatch(commit):
        raise ValueError("invalid toolchain build commit")
    tree = bundle / "root"
    files = payload(tree)
    # Check both header roots independently of the copy into the guest root.
    sources = {}
    for directory, headers in (("include", ROOT / "libs/libbreenix-libc/include"),
                               ("include-support", ROOT / "toolchain/include")):
        for header in headers.rglob("*"):
            if header.is_file():
                sources[DEV / directory / header.relative_to(headers)] = header
    sources[DEV / "linker.ld"] = ROOT / "userspace/programs/linker.ld"
    required = {SUITE, DEV / "lib/libc.a"} | {DEV / "bin" / tool for tool in TOOLS}
    if arch == "x86_64":
        required |= {DEV / "bin" / tool for tool in ("cc", "make", "as", "ld")}
        required.add(DEV / "lib/libinterop.a")
    corpus = ROOT / "toolchain/corpus"
    names = check_corpus(corpus)
    for name in names:
        for suffix in (".c", ".expect"):
            sources[CORPUS / (name + suffix)] = corpus / (name + suffix)
        for suffix in (".elf", ".inventory.json"):
            required.add(CORPUS / (name + suffix))
        required.add(CORPUS / "objs" / (name + ".o"))
    if arch == "x86_64":
        for name in ("hello.c", "hello.expect", "diagnostic.c"):
            path = Path("c-core") / name
            sources[CORPUS / path] = corpus / path
        for name in ("failure-status.mk", "graph-order.mk", "macro-precedence.mk",
                     "noop-when-current.mk", "sample.src", "suffix-inference.mk"):
            path = Path("make") / name
            sources[CORPUS / path] = corpus / path
        for name in ("Makefile", "main.c", "util.c", "util.h", "value.s", "project.expect"):
            path = Path("project") / name
            sources[CORPUS / path] = corpus / path
        for name in ("aggregates-exhaustion", "varargs", "sse-float", "x87-long-double"):
            path = Path("abi") / (name + ".expect")
            sources[CORPUS / path] = corpus / path
            for half in ("caller", "callee"):
                path = Path("abi") / f"{name}-{half}.c"
                sources[CORPUS / path] = corpus / path
                required.add(CORPUS / path.with_suffix(".o"))
        for category, names_in_category in (
                ("tls", ("template-init-zero-fill-aligned", "errno-per-thread", "tls-across-fork-exec")),
                ("interop", ("c-calls-rust", "rust-calls-c"))):
            for name in names_in_category:
                for suffix in (".c", ".expect"):
                    path = Path(category) / (name + suffix)
                    sources[CORPUS / path] = corpus / path
        sources[CORPUS / "tls/threads.h"] = corpus / "tls/threads.h"
        deferred = {"alignment", "atomics", "compound-literal", "designated-init", "generic"}
        candidates = set(names) - deferred
        if len(candidates) != 25:
            raise ValueError("unexpected C candidate corpus")
        required |= {CORPUS / "c-core/mac" / (name + ".elf") for name in candidates}
        actual = {path.name for path in (tree / CORPUS / "c-core/mac").iterdir()}
        if actual != {name + ".elf" for name in candidates}:
            raise ValueError("C candidate corpus differs from the selected cases")
        fixtures = corpus / "assembler"
        for source in fixtures.iterdir():
            if source.is_file():
                sources[CORPUS / "assembler" / source.name] = source
        for name in ("sse-exit", "x87-exit"):
            for suffix in (".elf", ".link.json"):
                required.add(CORPUS / "assembler" / (name + suffix))
    else:
        fixtures = corpus / "assembler-aarch64"
        for name in ("encodings.s", "encodings.hex", "bad-input.s", "fp-exit.s", "fp-exit.expect",
                     "call-exit.s", "call-exit.expect"):
            sources[CORPUS / "assembler" / name] = fixtures / name
        for name in ("fp-exit", "call-exit"):
            for suffix in (".elf", ".link.json"):
                required.add(CORPUS / "assembler" / (name + suffix))
    for name in LINKER_OBJECTS:
        required.add(CORPUS / "objs" / (name + ".o"))
    for name in ("got-main", "tls-template"):
        path = Path("linker") / (name + ".expect")
        sources[CORPUS / path] = corpus / path
    if missing := (sources.keys() | required) - set(files):
        raise ValueError(f"toolchain fixture omits {min(missing)}")
    for path, source in sources.items():
        if digest(tree / path) != digest(source):
            raise ValueError(f"staged bytes differ from the source for {path}")
    for name in names:
        receipt = json.loads((tree / CORPUS / (name + ".inventory.json")).read_text())
        obj = tree / CORPUS / "objs" / (name + ".o")
        if digest(obj) != receipt["object"]["sha256"]:
            raise ValueError(f"staged object differs from the reference link input: {name}")
    tools = json.loads(tool_record.read_text())
    fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
    interop = tools.pop("interop", None) if arch == "x86_64" else None
    expected_tools = set(TOOLS) | ({"bcc", "bmake"} if arch == "x86_64" else set())
    if set(tools) != expected_tools:
        raise ValueError("toolchain build record differs from the staged tool set")
    for name, tool in tools.items():
        filename = {"bcc": "cc", "bmake": "make"}.get(name, name)
        if (tool["cargoPackage"] != name or (name in ("bcc", "bmake") and tool.get("path") != f"bin/{filename}")
                or tool["target"] != f"{arch}-breenix"
                or tool["rustForkCommit"] != fork or not tool["rustc"]["version"]
                or not SHA256.fullmatch(tool["rustc"]["sha256"])
                or tool["sha256"] != digest(tree / DEV / "bin" / filename)):
            raise ValueError(f"{name}: build record differs from the staged tool")
    if arch == "x86_64":
        path = "lib/libinterop.a"
        if interop != {
                "cargoPackage": "interop", "path": path, "target": "x86_64-breenix-c",
                "rustForkCommit": fork, "rustc": tools["bcc"]["rustc"],
                "sha256": digest(tree / DEV / path)}:
            raise ValueError("interop build record differs from the staged archive")
        for tool, alias in (("bas", "as"), ("bld", "ld")):
            if digest(tree / DEV / "bin" / alias) != tools[tool]["sha256"]:
                raise ValueError(f"{alias}: staged driver tool differs from {tool}")
    (tree / MANIFEST).write_bytes(manifest_bytes(tree, files))
    record_data = dict(commit=commit, arch=arch, tools=tools,
                       manifestSha256=digest(tree / MANIFEST))
    if arch == "x86_64":
        record_data["candidateProducer"] = "mac-bcc" if sys.platform == "darwin" else "host-bcc"
        record_data["interop"] = interop
    record = json.dumps(record_data, indent=2) + "\n"
    (tree / RECORD).write_text(record)
    (bundle / "build.json").write_text(record)


def stat_path(debugfs, image, path):
    result = subprocess.run([debugfs, "-R", f"stat /{path}", str(image)], check=True, capture_output=True)
    if b"Inode:" in result.stdout:
        return result.stdout
    if b"File not found" not in result.stderr:
        raise ValueError(f"cannot inspect /{path}: {result.stderr.decode(errors='replace').strip()}")
    return None


def fixture_mode(path):
    return 0o755 if path == SUITE or path in TOOL_PATHS or path.suffix == ".elf" else 0o644


def install(debugfs, image, bundle, arch, scratch):
    tree = bundle / "root"
    files = payload(tree)
    record = json.loads((bundle / "build.json").read_text())
    if (not COMMIT.fullmatch(record.get("commit", "")) or record.get("arch") != arch
            or not SHA256.fullmatch(record.get("manifestSha256", ""))
            or (tree / RECORD).read_bytes() != (bundle / "build.json").read_bytes()
            or digest(tree / MANIFEST) != record["manifestSha256"]
            or (tree / MANIFEST).read_bytes() != manifest_bytes(tree, [p for p in files if p not in (MANIFEST, RECORD)])):
        raise ValueError("toolchain fixture files differ from their build record")

    # These paths are under clean roots. Create only missing ancestors, as
    # debugfs mkdir otherwise emits an error for shared ext2 layout directories.
    directories = sorted({parent for path in files for parent in path.parents if parent != Path(".")},
                         key=lambda path: (len(path.parts), path))
    commands = [f"mkdir /{path}\nset_inode_field /{path} mode 040755\n"
                for path in directories if stat_path(debugfs, image, path) is None]
    # Use paths relative to the trusted fixture tree for debugfs's host reads.
    for path in files:
        mode = f"0{0o100000 | fixture_mode(path):o}"
        commands.append(f"write {path} /{path}\nset_inode_field /{path} mode {mode}\n"
                        f"set_inode_field /{path} uid 0\nset_inode_field /{path} gid 0\n")
    command_file = scratch / "debugfs.commands"
    command_file.write_text("".join(commands))
    subprocess.run([debugfs, "-w", "-f", str(command_file), str(image)], cwd=tree,
                   check=True, capture_output=True)

    readback = scratch / "readback"
    readback.mkdir()
    for prefix in (*PREFIXES, SUITE):
        # Avoid extracting unrelated image files, including names the host
        # filesystem cannot represent.
        extracted = readback / prefix.parent
        extracted.mkdir(parents=True, exist_ok=True)
        subprocess.run([debugfs, "-R", f"rdump /{prefix} .", str(image)], cwd=extracted,
                       check=True, capture_output=True)
    if payload(readback) != files or manifest_bytes(readback, files) != manifest_bytes(tree, files):
        raise ValueError("staged toolchain fixture differs from its source tree")
    for path in files:
        if (readback / path).stat().st_mode & 0o777 != fixture_mode(path):
            raise ValueError(f"staged mode differs for /{path}")


def stage(base, output, suites, bundle, arch):
    seeds = {ROOT / directory / image for directory in ("target", "testdata")
             for image in ("ext2.img", "ext2-aarch64.img")}
    if (base == output or (output.exists() and base.samefile(output))
            or output.resolve() in {seed.resolve() for seed in seeds}):
        raise ValueError("suite staging requires a separate output image")
    debugfs, = require_e2fsprogs("debugfs")
    for path in (*PREFIXES, SUITE):
        if stat_path(debugfs, base, path) is not None:
            raise ValueError(f"ordinary seed contains /{path}")
    with tempfile.TemporaryDirectory(prefix=".suite-image-", dir=output.parent) as temporary:
        scratch = Path(temporary)
        image = scratch / "image.ext2"
        shutil.copyfile(base, image)
        if "toolchain" in suites.split(","):
            if bundle is None:
                raise ValueError("the toolchain suite needs --toolchain-bundle")
            install(debugfs, image, bundle, arch, scratch)
        image.replace(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-corpus").add_argument("corpus", type=Path)
    finalize = commands.add_parser("seal")
    finalize.add_argument("bundle", type=Path)
    finalize.add_argument("commit")
    finalize.add_argument("arch", choices=("x86_64", "aarch64"))
    finalize.add_argument("--tool-record", type=Path, required=True)
    copy = commands.add_parser("stage")
    copy.add_argument("base", type=Path)
    copy.add_argument("output", type=Path)
    copy.add_argument("--suite", default="")
    copy.add_argument("--toolchain-bundle", type=Path)
    copy.add_argument("--arch", choices=("x86_64", "aarch64"), required=True)
    args = parser.parse_args()
    try:
        if args.command == "check-corpus":
            check_corpus(args.corpus)
        elif args.command == "seal":
            seal(args.bundle, args.commit, args.arch, args.tool_record)
        else:
            stage(args.base.resolve(strict=True), args.output.absolute(), args.suite, args.toolchain_bundle, args.arch)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"toolchain image: {error}\n")


if __name__ == "__main__":
    main()
