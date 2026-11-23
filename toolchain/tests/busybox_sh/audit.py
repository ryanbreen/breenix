#!/usr/bin/env python3
"""Audit the pinned BusyBox phase commands against two sealed guest bundles."""

import argparse
import hashlib
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

from run import DRIVER, ROOT, VENDOR, run_phase_command

ARCHES = ("x86_64", "aarch64")
PHASES = (
    "unpack", "patch", "generators", "configure",
    *(f"build-libbb-{n}" for n in range(1, 5)),
    *(f"build-applets-{n}" for n in range(1, 4)),
    "link-and-list",
)
DEV = Path("opt/breenix-dev/25.2")
SHELL_WORDS = set(": . [ case cd command continue echo eval exit export for printf pwd read return "
                  "set shift test true false umask unset".split())
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=")
TRACE = re.compile(r"^\++ (\S.*)")
RECIPE_PREFIX = {"if", "elif", "then", "else", "while", "until", "do", "!", "{", "}"}


def commands(line):
    """Command boundaries of the pinned Kbuild recipes."""
    words = shlex.shlex(line, posix=True, punctuation_chars=True)
    words.whitespace_split = True
    words.commenters = ""
    simple = []
    for word in [*words, ";"]:
        if word and set(word) <= set(";|&()"):
            if simple:
                yield simple
            simple = []
        elif simple or (word not in RECIPE_PREFIX and not ASSIGNMENT.match(word)):
            simple.append(word)


def invoked_tools(argv, functions):
    command, *args = argv
    if command in SHELL_WORDS or command in functions or ASSIGNMENT.match(command):
        return
    if command == "exec":
        if args and args[0][0] not in "><" and not args[0][0].isdigit():
            yield from invoked_tools(args, functions)
        return
    yield command
    if command == "env":
        for index, arg in enumerate(args):
            if not arg.startswith("-") and not ASSIGNMENT.match(arg):
                yield from invoked_tools(args[index:], functions)
                break
    if command == "find":
        for index, arg in enumerate(args[:-1]):
            if arg in ("-exec", "-execdir"):
                yield from invoked_tools(args[index + 1:], functions)


def traced_tools(text, functions):
    for line in text.splitlines():
        if match := TRACE.fullmatch(line):
            command = match[1].split(None, 1)[0].strip("'\"")
            # xtrace may quote a multiline AWK program as one argument.
            if command == "find":
                argv = re.split(r"\s+", match[1])
            else:
                argv = shlex.split(match[1]) if command in ("env", "exec") else [command]
            yield from invoked_tools(argv, functions)


def unresolved_tools(bundle, applets, calls, tools, output, source, host_make):
    tree = bundle / "root"
    digests = dict(line.split("  ", 1)[::-1]
                   for line in (tree / DEV / "manifest.sha256").read_text().splitlines())
    missing = []
    for command in sorted(calls):
        path = Path(command)
        if command == str(host_make) or path.parent == tools:
            relative = DEV / "bin" / ("make" if command == str(host_make) else path.name)
            staged = tree / relative
            if not (staged.is_file() and os.access(staged, os.X_OK)
                    and hashlib.sha256(staged.read_bytes()).hexdigest() == digests.get(str(relative))):
                missing.append(f"{relative} (sealed executable)")
        elif (path == output / "hostcc" or (not path.is_absolute() and path.parent != Path("."))
              or path.is_relative_to(source)):
            generated = path if path.is_absolute() else source / path
            if not (generated.is_file() and os.access(generated, os.X_OK)):
                missing.append(f"{command} (workload script or generated executable)")
        else:
            applet = applets / path.name
            if not (applet.is_file() and os.access(applet, os.X_OK)
                    and (not path.is_absolute() or path.parent == Path("/bin"))):
                missing.append(f"{command} (image BusyBox applet)")
    return missing


def capture(directory, arch, sealed_bundle, host_tools):
    fixture = directory / "fixture"
    shutil.copytree(ROOT / VENDOR, fixture / VENDOR)
    bundle = fixture / "bundle"
    tools = bundle / "bin"
    tools.mkdir(parents=True)
    host_make = host_tools / "bmake"
    for name, package in {"cc": "bcc", "as": "bas", "ld": "bld", "ar": "bar", "nm": "bnm"}.items():
        shutil.copy2(host_tools / package, tools / name)
    for name in ("include", "include-support", "lib", "linker.ld"):
        staged = sealed_bundle / "root" / DEV / name
        if staged.is_dir():
            shutil.copytree(staged, bundle / name)
        else:
            shutil.copy2(staged, bundle / name)

    # Record the guest binding before building the predecessor files on the Mac.
    # The host execution uses native Clang for runnable generators and the x86
    # toolchain for target objects used only as prerequisite files by this check.
    output = directory / "build"
    dry = directory / "dry"
    dry.mkdir()
    clang = os.environ["BREENIX_TOOLCHAIN_CLANG"]
    (tools / "make").write_text(f"""#!{sys.executable}
import os, pathlib, subprocess, sys, tempfile
args = sys.argv[1:]
assert 'ARCH=' + {arch!r} in args
bound = args if '-n' in args else ['-n', *args]
with tempfile.NamedTemporaryFile(dir={str(dry)!r}, prefix={arch!r} + '-', delete=False) as trace:
    result = subprocess.run([{str(host_make)!r}, *bound], stdout=trace)
    if result.returncode:
        sys.exit(result.returncode)
if '-n' in args:
    sys.stdout.buffer.write(pathlib.Path(trace.name).read_bytes())
else:
    native = []
    for arg in args:
        if arg.startswith('HOSTCC='):
            arg = 'HOSTCC=' + {str(clang)!r}
        elif arg.startswith('CC='):
            arg = arg.replace('--target=aarch64', '--target=x86_64')
        native.append(arg)
    os.execv({str(host_make)!r}, [{str(host_make)!r}, *native])
""")
    (tools / "make").chmod(0o755)
    env = {"PATH": os.defpath, "LC_ALL": "C", "BUSYBOX_JOBS": "8"}
    source = output / "busybox-1.37.0"
    for phase in PHASES:
        result = run_phase_command(
            ["/bin/sh", "-x", str(DRIVER), phase, str(fixture), str(output), arch],
            output, env,
        )
        assert result.returncode == 0, f"{phase}: {result.stderr[-5000:]}"
        (directory / f"{phase}.trace").write_text(result.stderr)


def phase_tools(directory, arch, host_make):
    tools = directory / "fixture/bundle/bin"
    output = directory / "build"
    source = output / "busybox-1.37.0"
    functions = set(re.findall(r"^(\w+)\(\) \{", DRIVER.read_text(), re.M)) | {"trace_hostcc"}
    host_calls = {"/bin/sh"}
    for phase in PHASES:
        host_calls.update(traced_tools((directory / f"{phase}.trace").read_text(), functions))
    for name in ("patch.log", "config.log"):
        host_calls.update(traced_tools((output / name).read_text(), functions))

    # Follow the generated HOSTCC wrapper to bcc and its sibling tools.
    hostcc = output / "hostcc"
    host_calls.update(traced_tools("+ " + hostcc.read_text().splitlines()[-1], set()))
    host_calls.update(str(tools / name) for name in ("as", "ld"))

    shell_scripts = set()
    found_hostcc = False
    compile_goals = set()
    for trace in sorted((directory / "dry").glob(arch + "-*")):
        for line in trace.read_text().splitlines():
            for argv in commands(line):
                host_calls.update(invoked_tools(argv, set()))
                found_hostcc |= argv[0] == str(hostcc)
                if argv[0] == str(tools / "cc") and "-c" in argv and "-o" in argv:
                    assert f"--target={arch}" in argv, argv[:4]
                    compile_goals.add(argv[argv.index("-o") + 1])
                path = source / argv[0]
                if path != hostcc and path.is_file():
                    with path.open("rb") as executable:
                        if executable.read(10) == b"#!/bin/sh\n":
                            shell_scripts.add(tuple(argv))
    assert found_hostcc and f"--target={arch}" in hostcc.read_text(), f"{arch}: no guest HOSTCC recipes"
    expected = {"applets/applets.o", *(output / "libbb.goals").read_text().split(),
                *(output / "applets.goals").read_text().split()}
    assert expected <= compile_goals, f"{arch}: dry runs missed {sorted(expected - compile_goals)}"
    for command in sorted(shell_scripts):
        result = subprocess.run(["/bin/sh", "-x", *command], cwd=source,
                                env={"PATH": os.defpath, "LC_ALL": "C"},
                                check=True, capture_output=True, text=True, timeout=8)
        functions = set(re.findall(r"^\s*(\w+)\(\)\s*\{", (source / command[0]).read_text(), re.M))
        host_calls.update(traced_tools(result.stderr, functions))
    print(f"{arch}: {len(compile_goals)} object recipes, {len(host_calls)} command paths")
    return host_calls, tools, output, source, host_make


def every_invoked_tool_is_in_the_sealed_bundle(directory, bundles, host_tools):
    missing = {}
    for arch, staged in zip(ARCHES, bundles, strict=True):
        staged_driver = staged / "root/usr/share/breenix/toolchain/busybox/busybox.sh"
        assert staged_driver.read_bytes() == DRIVER.read_bytes(), f"{arch}: mismatched BusyBox driver"
        calls = phase_tools((directory / arch).resolve(), arch, host_tools / "bmake")
        image_dir = ROOT / "userspace/programs"
        if arch == "aarch64":
            image_dir /= arch
        with tempfile.TemporaryDirectory(dir=directory) as installed:
            image = Path(installed)
            subprocess.run([ROOT / "scripts/install-libc-busybox.sh", image_dir,
                            ROOT / VENDOR / "required-applets", image], check=True, timeout=5)
            missing[arch] = unresolved_tools(staged, image / "bin", *calls)
    assert not any(missing.values()), f"unresolved BusyBox commands: {missing}"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("capture", "check"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("host_tools", type=Path)
    parser.add_argument("x86_64_bundle", type=Path)
    parser.add_argument("aarch64_bundle", type=Path, nargs="?")
    parser.add_argument("--arch", choices=ARCHES, default="x86_64", help="target for capture")
    args = parser.parse_args()
    directory, host_tools, x86 = (p.resolve() for p in (args.directory, args.host_tools, args.x86_64_bundle))
    if args.mode == "capture":
        capture(directory / args.arch, args.arch, x86, host_tools)
    else:
        if args.aarch64_bundle is None:
            parser.error("check needs both sealed bundles")
        every_invoked_tool_is_in_the_sealed_bundle(directory, (x86, args.aarch64_bundle.resolve()), host_tools)
