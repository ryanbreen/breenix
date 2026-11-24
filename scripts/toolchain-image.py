#!/usr/bin/env python3
"""Seal and stage the toolchain suite's files on a private ext2 work copy."""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
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
BUSYBOX = CORPUS / "busybox"
BUSYBOX_MILESTONE = "toolchain/busybox-on-box"
BUSYBOX_SCRATCH = "toolchain-scratch.img"
KERNEL_ASSEMBLY_MILESTONE = "toolchain/kernel-assembly"
KERNEL_ASSEMBLY = CORPUS / "kernel-asm"
KERNEL_ASSEMBLY_RECORD = "kernel-assembly.record"
KERNEL_ASM_SOURCES = (
    "src/syscall/entry.asm",
    "src/interrupts/timer_entry.asm",
    "src/interrupts/breakpoint_entry.asm",
    "src/arch_impl/x86_64/ap_trampoline.asm",
)
KERNEL_ASM_OBJECTS = ("syscall_entry.o", "timer_entry.o", "breakpoint_entry.o", "ap_trampoline.o")
# 25.11d2-driver measured 39,576 KiB. E24's 64 MiB scratch geometry
# leaves 25,960 KiB for filesystem overhead and guest allocation differences.
BUSYBOX_SCRATCH_BYTES = 64 << 20
SUITE = Path("sbin/suite-toolchain")
LIBEXEC = Path("usr/libexec/breenix")
TOOLS = ("bas", "bld", "bar", "bnm")
DRIVER_TOOL_ALIASES = (("bas", "as"), ("bld", "ld"), ("bar", "ar"), ("bnm", "nm"))
TOOL_PATHS = {DEV / "bin" / tool for tool in (*TOOLS, "cc", "make")}
TOOL_PATHS.update(DEV / "bin" / alias for _, alias in DRIVER_TOOL_ALIASES)
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


def c11_inputs(arch):
    source = ROOT / "toolchain/corpus/c11"
    constraints = json.loads((source / "constraints.json").read_text())
    programs = ("declarations-initializers", "generic-static-assert-alignment",
                "floating", "atomics", "profile-macros")
    names = ["constraints.json", "constraint-diagnostics.expect", *constraints["guest"].values(),
             *(f"{name}.{suffix}" for name in programs for suffix in ("c", "expect"))]
    inputs = {Path("c11") / name: source / name for name in names}
    if arch == "aarch64":
        inputs[Path("c11/floating.c")] = ROOT / "toolchain/corpus/c11-aarch64/floating.c"
    return inputs


def stage_c11(corpus, arch):
    for path, source in c11_inputs(arch).items():
        destination = corpus / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)


def c17_libc_inputs():
    inputs = {Path("c17") / p.name: p for p in (ROOT / "toolchain/corpus/c17").iterdir() if p.is_file()}
    inputs[Path("libc-suite.mk")] = ROOT / "toolchain/corpus/libc-suite.mk"
    inputs.update({Path("libc-suite") / p.name: p for p in (ROOT / "userspace/libc-suite").iterdir()
                   if p.suffix in (".c", ".h") or p.name == "categories"})
    inputs[Path("libc-suite/libc.json")] = ROOT / "docs/suites/libc.json"
    return inputs


def libc_references():
    source = ROOT / "userspace/libc-suite/categories"
    categories = [line.split()[0] for line in source.read_text().splitlines()
                  if line.strip() and not line.lstrip().startswith("#")]
    if not categories or len(set(categories)) != len(categories) or any(not NAME.fullmatch(c) for c in categories):
        raise ValueError(f"invalid category list in {source}")
    return tuple(LIBEXEC / f"libc-{category}_test.elf" for category in categories)


def stage_c17_libc(tree, binaries, arch, clang):
    corpus = tree / CORPUS
    for path, source in c17_libc_inputs().items():
        destination = corpus / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    for path in libc_references():
        destination = tree / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(binaries / path.name, destination)

    # Use the same declaration inventory as the reference suite's provenance audit.
    spec = spec_from_file_location("libc_symbol_audit", ROOT / "scripts/libc-symbol-audit.py")
    audit = module_from_spec(spec)
    spec.loader.exec_module(audit)
    compiler = [str(clang), "--no-default-config", f"--target={arch}-unknown-linux-gnu",
                "-Ulinux", "-U__linux", "-U__linux__", "-U__gnu_linux__", "-D__breenix__=1",
                "-nostdinc", "-isystem", str(audit.INCLUDE),
                "-isystem", str(ROOT / "toolchain/include")]
    functions = audit.declared_functions(arch, audit.public_headers(), compiler=compiler)
    if not functions:
        raise ValueError("empty libc declaration inventory")
    (corpus / "libc-functions.txt").write_text("".join(f"{name}\n" for name in sorted(functions)))


def payload(tree):
    files = []
    references = libc_references()
    for path in sorted(tree.rglob("*")):
        relative = path.relative_to(tree)
        if path.is_symlink() or not GUEST_PATH.fullmatch(relative.as_posix()):
            raise ValueError(f"invalid fixture path: {relative}")
        if path.is_dir():
            if not any(relative.is_relative_to(prefix) or relative in prefix.parents
                       for prefix in (DEV, CORPUS, SUITE, *references)):
                raise ValueError(f"unexpected fixture directory: {relative}")
        elif path.is_file() and (relative == SUITE or relative in references or
                                any(relative.is_relative_to(prefix) for prefix in (DEV, CORPUS))):
            files.append(relative)
        else:
            raise ValueError(f"unexpected fixture file: {relative}")
    if SUITE not in files or not all((tree / prefix).is_dir() for prefix in (DEV, CORPUS)):
        raise ValueError("the fixture needs both payload prefixes and /sbin/suite-toolchain")
    return files


def manifest_bytes(tree, files):
    return "".join(f"{digest(tree / path)}  {path.as_posix()}\n" for path in files).encode()


def busybox_inputs():
    vendor = ROOT / "vendor/busybox"
    return {
        BUSYBOX / "busybox.sh": ROOT / "toolchain/corpus/busybox.sh",
        BUSYBOX / "mac-applets.txt": ROOT / "toolchain/corpus/busybox-mac-applets.txt",
        **{BUSYBOX / path.relative_to(ROOT): path for path in (
            vendor / "busybox-1.37.0.tar.bz2", vendor / "breenix-libc.config",
            *sorted((vendor / "patches").iterdir()),
        )},
    }


def format_busybox_scratch(path):
    # Use the same native formatter as breenfs-suite-fixtures.sh (E24 24.20).
    tools = ROOT / "tools/breenfs"
    target = tools / "target"
    subprocess.run(["cargo", "build", "--locked", "--release", "--quiet", "--bin", "breenfs-mkfs",
                    "--target-dir", str(target)], cwd=tools, check=True)
    with tempfile.TemporaryDirectory(prefix="busybox-scratch-") as contents:
        (Path(contents) / "busybox-scratch").write_bytes(b"toolchain/busybox-on-box\n")
        subprocess.run([str(target / "release/breenfs-mkfs"), "--size",
                        str(BUSYBOX_SCRATCH_BYTES), "--populate", contents, str(path)], check=True)


def busybox_scratch_record(path):
    if path.is_symlink() or path.stat().st_size != BUSYBOX_SCRATCH_BYTES:
        raise ValueError("invalid BusyBox scratch image")
    return {"size": BUSYBOX_SCRATCH_BYTES, "sha256": digest(path)}


def read_kernel_assembly_record(path):
    fields = {}
    for line in path.read_text().splitlines():
        name, separator, value = line.partition("=")
        if not separator or name in fields or not value:
            raise ValueError("invalid kernel assembly record field")
        fields[name] = value
    digests = (*KERNEL_ASM_SOURCES, *KERNEL_ASM_OBJECTS, "bas_sha256", "kernel_elf_sha256")
    if (fields.keys() != {*digests, "cargo_package", "build_host", "rustc"}
            or fields["cargo_package"] != "bas"
            or any(not SHA256.fullmatch(fields[name]) for name in digests)):
        raise ValueError("invalid kernel assembly record")
    return fields


def kernel_assembly_record(tree, files, arch, milestone):
    staged = {path for path in files if path.is_relative_to(KERNEL_ASSEMBLY)}
    if milestone != KERNEL_ASSEMBLY_MILESTONE:
        if staged:
            raise ValueError(f"kernel assembly fixture requires {KERNEL_ASSEMBLY_MILESTONE}")
        return None
    if arch != "x86_64":
        raise ValueError(f"{KERNEL_ASSEMBLY_MILESTONE} requires x86_64")
    inputs = (*KERNEL_ASM_SOURCES, *KERNEL_ASM_OBJECTS)
    if staged != {KERNEL_ASSEMBLY / name for name in (*inputs, KERNEL_ASSEMBLY_RECORD)}:
        raise ValueError("kernel assembly fixture differs from the required file set")
    directory = tree / KERNEL_ASSEMBLY
    fields = read_kernel_assembly_record(directory / KERNEL_ASSEMBLY_RECORD)
    for name in inputs:
        if digest(directory / name) != fields[name]:
            raise ValueError(f"staged kernel assembly bytes differ from the record for {name}")
    return fields


def seal(bundle, commit, arch, tool_record, milestone=None):
    if not COMMIT.fullmatch(commit):
        raise ValueError("invalid toolchain build commit")
    tree = bundle / "root"
    files = payload(tree)
    assembly = kernel_assembly_record(tree, files, arch, milestone)
    # Check both header roots independently of the copy into the guest root.
    sources = {}
    for directory, headers in (("include", ROOT / "libs/libbreenix-libc/include"),
                               ("include-support", ROOT / "toolchain/include")):
        for header in headers.rglob("*"):
            if header.is_file():
                sources[DEV / directory / header.relative_to(headers)] = header
    sources[DEV / "linker.ld"] = ROOT / "userspace/programs/linker.ld"
    required = {SUITE, DEV / "lib/libc.a"} | {DEV / "bin" / tool for tool in TOOLS}
    required |= {DEV / "bin" / tool for tool in ("cc", "make")}
    required.update(DEV / "bin" / alias for _, alias in DRIVER_TOOL_ALIASES)
    archive_targets = {
        "interop": "x86_64-breenix-c" if arch == "x86_64" else "aarch64-breenix",
        "brt": f"{arch}-breenix",
    }
    required |= {DEV / "lib" / f"lib{name}.a" for name in archive_targets}
    corpus = ROOT / "toolchain/corpus"
    names = check_corpus(corpus)
    for name in names:
        for suffix in (".c", ".expect"):
            sources[CORPUS / (name + suffix)] = corpus / (name + suffix)
        for suffix in (".elf", ".inventory.json"):
            required.add(CORPUS / (name + suffix))
        required.add(CORPUS / "objs" / (name + ".o"))
    for name in ("hello.c", "hello.expect", "diagnostic.c"):
        path = Path("c-core") / name
        sources[CORPUS / path] = corpus / path
    for name in ("failure-status.mk", "graph-order.mk", "macro-precedence.mk",
                 "noop-when-current.mk", "sample.src", "suffix-inference.mk"):
        path = Path("make") / name
        sources[CORPUS / path] = corpus / path
    for name in ("Makefile", "main.c", "util.c", "util.h", "value.s", "project.expect"):
        path = Path("project") / name
        sources[CORPUS / path] = (corpus / "project-aarch64/value.s"
                                  if arch == "aarch64" and name == "value.s" else corpus / path)
    candidates = set(names)
    if len(candidates) != 30:
        raise ValueError("unexpected C candidate corpus")
    required |= {CORPUS / "c-core/mac" / (name + ".elf") for name in candidates}
    actual = {path.name for path in (tree / CORPUS / "c-core/mac").iterdir()}
    if actual != {name + ".elf" for name in candidates}:
        raise ValueError("C candidate corpus differs from the selected cases")
    abi_dir = "abi" if arch == "x86_64" else "abi-aarch64"
    for name in ("aggregates-exhaustion", "varargs", "sse-float", "x87-long-double"):
        for suffix in ((".expect", ".h") if arch == "aarch64" else (".expect",)):
            path = Path(abi_dir) / (name + suffix)
            sources[CORPUS / path] = corpus / path
        for half in ("caller", "callee"):
            path = Path(abi_dir) / f"{name}-{half}.c"
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
    sources.update({CORPUS / path: source for path, source in c11_inputs(arch).items()})
    sources.update({CORPUS / path: source for path, source in c17_libc_inputs().items()})
    required.update((*libc_references(), CORPUS / "libc-functions.txt"))

    busybox = busybox_inputs() if milestone == BUSYBOX_MILESTONE else {}
    if {path for path in files if path.is_relative_to(BUSYBOX)} != busybox.keys():
        raise ValueError("BusyBox inputs differ from the selected fixture file set")
    sources.update(busybox)

    if arch == "x86_64":
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
    archives = {name: tools.pop(name, None) for name in archive_targets}
    expected_tools = set(TOOLS) | {"bcc", "bmake"}
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
    for name, target in archive_targets.items():
        path = f"lib/lib{name}.a"
        if archives[name] != {
                "cargoPackage": name, "path": path, "target": target,
                "rustForkCommit": fork, "rustc": tools["bcc"]["rustc"],
                "sha256": digest(tree / DEV / path)}:
            raise ValueError(f"{name}: build record differs from the staged archive")
    for tool, alias in DRIVER_TOOL_ALIASES:
        if digest(tree / DEV / "bin" / alias) != tools[tool]["sha256"]:
            raise ValueError(f"{alias}: staged driver tool differs from {tool}")
    (tree / MANIFEST).write_bytes(manifest_bytes(tree, files))
    record_data = dict(commit=commit, arch=arch, tools=tools,
                       manifestSha256=digest(tree / MANIFEST))
    record_data["candidateProducer"] = "mac-bcc" if sys.platform == "darwin" else "host-bcc"
    record_data.update(archives)
    if busybox:
        record_data["busyboxInputs"] = {str(path): digest(tree / path) for path in busybox}
        scratch = bundle / BUSYBOX_SCRATCH
        format_busybox_scratch(scratch)
        record_data["busyboxScratch"] = busybox_scratch_record(scratch)
    if assembly is not None:
        record_data["kernelAssembly"] = assembly
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


def install(debugfs, image, bundle, arch, scratch, milestone=None):
    tree = bundle / "root"
    files = payload(tree)
    references = libc_references()
    record = json.loads((bundle / "build.json").read_text())
    busybox = {path for path in files if path.is_relative_to(BUSYBOX)}
    disk = bundle / BUSYBOX_SCRATCH
    if milestone == BUSYBOX_MILESTONE:
        if (busybox != busybox_inputs().keys()
                or record.get("busyboxInputs") != {str(path): digest(tree / path) for path in busybox}
                or record.get("busyboxScratch") != busybox_scratch_record(disk)):
            raise ValueError("BusyBox fixture differs from its build record")
    elif busybox or "busyboxInputs" in record or "busyboxScratch" in record or disk.exists() or disk.is_symlink():
        raise ValueError("BusyBox fixture requires toolchain/busybox-on-box")
    assembly = kernel_assembly_record(tree, files, arch, milestone)
    if ((assembly is None and "kernelAssembly" in record)
            or (assembly is not None and record.get("kernelAssembly") != assembly)):
        raise ValueError("kernel assembly fixture differs from its build record")
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
        # An image may already hold the exact reference name. Readback below
        # checks its bytes and mode against the reference in the bundle.
        if path in references and stat_path(debugfs, image, path) is not None:
            continue
        mode = f"0{0o100000 | fixture_mode(path):o}"
        commands.append(f"write {path} /{path}\nset_inode_field /{path} mode {mode}\n"
                        f"set_inode_field /{path} uid 0\nset_inode_field /{path} gid 0\n")
    command_file = scratch / "debugfs.commands"
    command_file.write_text("".join(commands))
    subprocess.run([debugfs, "-w", "-f", str(command_file), str(image)], cwd=tree,
                   check=True, capture_output=True)

    readback = scratch / "readback"
    readback.mkdir()
    for prefix in (*PREFIXES, SUITE, *references):
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


def stage(base, output, suites, bundle, arch, milestone=None):
    seeds = {ROOT / directory / image for directory in ("target", "testdata")
             for image in ("ext2.img", "ext2-aarch64.img")}
    if (base == output or (output.exists() and base.samefile(output))
            or output.resolve() in {seed.resolve() for seed in seeds}):
        raise ValueError("suite staging requires a separate output image")
    toolchain = "toolchain" in suites.split(",")
    if milestone in (BUSYBOX_MILESTONE, KERNEL_ASSEMBLY_MILESTONE) and not toolchain:
        raise ValueError(f"{milestone} requires the toolchain suite")
    debugfs, = require_e2fsprogs("debugfs")
    for path in (*PREFIXES, SUITE):
        if stat_path(debugfs, base, path) is not None:
            raise ValueError(f"ordinary seed contains /{path}")
    with tempfile.TemporaryDirectory(prefix=".suite-image-", dir=output.parent) as temporary:
        scratch = Path(temporary)
        image = scratch / "image.ext2"
        shutil.copyfile(base, image)
        if toolchain:
            if bundle is None:
                raise ValueError("the toolchain suite needs --toolchain-bundle")
            install(debugfs, image, bundle, arch, scratch, milestone)
        image.replace(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-corpus").add_argument("corpus", type=Path)
    c17 = commands.add_parser("stage-c17-libc")
    c17.add_argument("tree", type=Path)
    c17.add_argument("binaries", type=Path)
    c17.add_argument("arch", choices=("x86_64", "aarch64"))
    c17.add_argument("clang", type=Path)
    c11 = commands.add_parser("stage-c11")
    c11.add_argument("corpus", type=Path)
    c11.add_argument("arch", choices=("x86_64", "aarch64"))
    finalize = commands.add_parser("seal")
    finalize.add_argument("bundle", type=Path)
    finalize.add_argument("commit")
    finalize.add_argument("arch", choices=("x86_64", "aarch64"))
    finalize.add_argument("--tool-record", type=Path, required=True)
    finalize.add_argument("--milestone")
    copy = commands.add_parser("stage")
    copy.add_argument("base", type=Path)
    copy.add_argument("output", type=Path)
    copy.add_argument("--suite", default="")
    copy.add_argument("--toolchain-bundle", type=Path)
    copy.add_argument("--arch", choices=("x86_64", "aarch64"), required=True)
    copy.add_argument("--milestone")
    args = parser.parse_args()
    try:
        if args.command == "check-corpus":
            check_corpus(args.corpus)
        elif args.command == "stage-c17-libc":
            stage_c17_libc(args.tree, args.binaries, args.arch, args.clang)
        elif args.command == "stage-c11":
            stage_c11(args.corpus, args.arch)
        elif args.command == "seal":
            seal(args.bundle, args.commit, args.arch, args.tool_record, args.milestone)
        else:
            stage(args.base.resolve(strict=True), args.output.absolute(), args.suite,
                  args.toolchain_bundle, args.arch, args.milestone)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"toolchain image: {error}\n")


if __name__ == "__main__":
    main()
