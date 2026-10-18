#!/usr/bin/env python3
"""Remove compiler_builtins' libc math code, preserving compiler runtime helpers.

Run after EACH libc.a build, before any Rust or C link. Object members mix compiler
helpers and math, so deleting entire members would delete required ABI helpers.
Instead remove the math function/data sections and their symbols, then audit all
remaining symbols (including undefined references). Never read or translate their
implementation; this tool uses object metadata only.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile

MEMORY = {'memcpy', 'memmove', 'memset', 'memcmp', 'bcmp', 'strlen'}


def symbols(nm, path):
    result = subprocess.check_output([str(nm), '--format=posix', str(path)], text=True,
                                     stderr=subprocess.DEVNULL)
    return [(f[0], f[1]) for line in result.splitlines() if len(f := line.split()) >= 2]


def foreign(name):
    return 'libm_math' in name or 'compiler_builtins4math' in name or 'compiler_builtins..math..' in name


def clean(archive, tool_dir):
    before = hashlib.sha256(archive.read_bytes()).digest()
    timestamps = archive.stat()
    record = archive.with_suffix('.a.cleaned.json')
    previous = json.loads(record.read_text()) if record.exists() else None
    nm, ar, objcopy = [tool_dir / name for name in ('llvm-nm', 'llvm-ar', 'llvm-objcopy')]
    # rustc's llvm-tools ship llvm-readobj without the llvm-readelf name; its GNU style is readelf.
    readelf = [str(tool_dir / 'llvm-readelf')] if (tool_dir / 'llvm-readelf').exists() else \
        [str(tool_dir / 'llvm-readobj'), '--elf-output-style=GNU']
    members = subprocess.check_output([str(ar), 't', str(archive)], text=True).splitlines()
    removed = 0
    with tempfile.TemporaryDirectory(prefix='breenix-libc-clean-') as tmp:
        directory = Path(tmp)
        for member in members:
            if not member.startswith('compiler_builtins'):
                continue
            subprocess.run([str(ar), 'x', '--output', tmp, str(archive), member], check=True)
            obj = directory / member
            syms = symbols(nm, obj)
            names = {name for name, kind in syms if foreign(name) or
                     (kind in 'Ww' and not name.startswith('_') and name not in MEMORY)}
            if not names:
                continue
            # Embedded bitcode (.llvmbc, .llvmcmd) is the member's whole module, math included.
            command = [str(objcopy), '--strip-debug', '--wildcard',
                       '--remove-section=*libm_math*', '--remove-section=*compiler_builtins4math*',
                       '--remove-section=*compiler_builtins..math..*',
                       '--remove-section=.llvmbc', '--remove-section=.llvmcmd']
            metadata = subprocess.check_output(readelf + ['-SW', '-sW', str(obj)], text=True)
            sections = dict(re.findall(r'^\s*\[\s*(\d+)\]\s+(\S+)', metadata, re.MULTILINE))
            for line in metadata.splitlines():
                fields = line.split()
                if len(fields) == 8 and fields[-1] in names and fields[-2] in sections:
                    command.append('--remove-section=' + sections[fields[-2]])
            subprocess.run(command + [str(obj)], check=True)
            # ELF may keep an unreferenced undefined symbol after its caller's
            # section is removed. Strip it; objcopy fails if a relocation still
            # uses it, so this cannot disguise a live dependency.
            dangling = [name for name, kind in symbols(nm, obj) if kind == 'U' and foreign(name)]
            if dangling:
                subprocess.run([str(objcopy)] + ['--strip-symbol=' + name for name in dangling] + [str(obj)], check=True)
            remaining = {name for name, kind in symbols(nm, obj) if kind != 'U' and (foreign(name) or name in names)}
            if remaining:
                raise SystemExit('libc-clean-archive: unresolved foreign math symbols: ' + ', '.join(sorted(remaining)))
            subprocess.run([str(ar), 'r', str(archive), str(obj)], check=True)
            removed += len(names)
        subprocess.run([str(ar), 's', str(archive)], check=True)
    # Cargo can republish its original staticlib even on a fresh build. Remember
    # the *cleaned* bytes across invocations, not just the bytes before this call,
    # so republishing then cleaning identical libc preserves dependent link freshness.
    after = hashlib.sha256(archive.read_bytes()).digest()
    if previous and previous['sha256'] == after.hex():
        os.utime(archive, ns=(timestamps.st_atime_ns, previous['mtime_ns']))
    elif after == before:
        os.utime(archive, ns=(timestamps.st_atime_ns, timestamps.st_mtime_ns))
    temporary = record.with_suffix('.tmp')
    temporary.write_text(json.dumps({'sha256': after.hex(), 'mtime_ns': archive.stat().st_mtime_ns}) + '\n')
    temporary.replace(record)
    print(f'  libc-clean-archive: removed {removed} foreign math symbols')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--tool-dir', type=Path, required=True)
    args = parser.parse_args()
    clean(args.archive.resolve(), args.tool_dir.resolve())


if __name__ == '__main__':
    main()
