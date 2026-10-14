#!/usr/bin/env python3
"""Fail when a function Breenix's C headers declare comes only from compiler_builtins.

  libc-symbol-audit.py --arch aarch64|x86_64

Every function declared in libs/libbreenix-libc/include must be defined strongly by
Breenix's own code in libc.a, or not at all (the suite build stands in for functions
not written yet). A definition that is only weak, or only in a compiler_builtins
member, fails: compiler_builtins' math comes from a port of another C library, and
its weak mem* and str* would silently stand in for ours. Compiler runtime helpers
(__addtf3, __udivti3) are not declared in the headers and are not checked.

The declarations come from clang's AST of a translation unit that includes every
public header with _GNU_SOURCE, so a header that does not compile on its own also
fails the audit.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
INCLUDE = ROOT / 'libs/libbreenix-libc/include'
LIBC = {
    'aarch64': ROOT / 'libs/libbreenix-libc/target/aarch64-breenix/release/libc.a',
    'x86_64': ROOT / 'libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a',
}
CC = ROOT / 'scripts/breenix-cc'


def public_headers():
    return sorted(str(path.relative_to(INCLUDE)) for path in INCLUDE.rglob('*.h')
                  if path.relative_to(INCLUDE).parts[0] != '__breenix')


class Declarations:
    """Walks clang's JSON AST in output order: a location omits its file when it is the
    same as the previous location printed, so the current file is tracked as it goes."""

    def __init__(self):
        self.file = None
        self.functions = {}

    def location(self, loc):
        if not isinstance(loc, dict):
            return
        if 'spellingLoc' in loc or 'expansionLoc' in loc:
            self.location(loc.get('spellingLoc'))
            self.location(loc.get('expansionLoc'))
            return
        if 'file' in loc:
            self.file = loc['file']

    def walk(self, node, top):
        file_at_decl = None
        for key, value in node.items():
            if key == 'loc':
                self.location(value)
                file_at_decl = self.file
            elif key == 'range':
                self.location(value.get('begin'))
                self.location(value.get('end'))
            elif key == 'inner':
                for child in value:
                    self.walk(child, False)
        if (top and node.get('kind') == 'FunctionDecl' and not node.get('isImplicit')
                and node.get('storageClass') != 'static' and file_at_decl
                and Path(file_at_decl).resolve().is_relative_to(INCLUDE.resolve())):
            self.functions.setdefault(node['name'], file_at_decl)


def declared_functions(arch, headers):
    source = ''.join(f'#include <{header}>\n' for header in headers)
    result = subprocess.run(
        [str(CC), '--arch', arch, '-std=gnu17', '-D_GNU_SOURCE', '-fsyntax-only',
         '-Xclang', '-ast-dump=json', '-x', 'c', '-'],
        input=source.encode(), capture_output=True)
    if result.returncode != 0:
        sys.exit(f'libc-symbol-audit: the headers do not compile together for {arch}:\n'
                 + result.stderr.decode(errors='replace'))
    walker = Declarations()
    for child in json.loads(result.stdout).get('inner', []):
        walker.walk(child, True)
    return walker.functions


def definitions(arch, path=None):
    """name -> list of (member, weak) for every definition in libc.a."""
    tool_dir = subprocess.check_output([str(CC), '--arch', arch, '--tool-dir'], text=True).strip()
    nm = Path(tool_dir) / 'llvm-nm'
    output = subprocess.check_output([str(nm), '-A', str(path or LIBC[arch])],
                                     text=True, stderr=subprocess.DEVNULL)
    found = {}
    for line in output.splitlines():
        # <archive>:<member>: <address> <type> <name>
        head, _, rest = line.rpartition(': ')
        fields = rest.split()
        if len(fields) < 2:
            continue
        kind, name = fields[-2], fields[-1]
        if kind == 'U' and not ('libm_math' in name or 'compiler_builtins4math' in name or 'compiler_builtins..math..' in name):
            continue
        member = head.rsplit(':', 1)[-1]
        found.setdefault(name, []).append((member, kind in 'wWvV'))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--arch', choices=sorted(LIBC), required=True)
    parser.add_argument('--elf', type=Path, help='also reject foreign or weak math in a linked C ELF')
    args = parser.parse_args()
    if not LIBC[args.arch].is_file():
        sys.exit(f'libc-symbol-audit: {LIBC[args.arch]} is missing')
    headers = public_headers()
    functions = declared_functions(args.arch, headers)
    defined = definitions(args.arch)
    problems = []
    for name, defs in sorted(defined.items()):
        if ('libm_math' in name or 'compiler_builtins4math' in name or 'compiler_builtins..math..' in name
                or any(member.startswith('compiler_builtins') for member, _ in defs)
                and (name in functions or any(member.startswith('compiler_builtins') and weak for member, weak in defs))
                and not name.startswith('_')
                and name not in {'memcpy', 'memmove', 'memset', 'memcmp', 'bcmp', 'strlen'}):
            problems.append(f'  forbidden foreign math symbol {name}')
    for name in sorted(functions):
        defs = defined.get(name)
        if not defs:
            continue
        ours = [member for member, weak in defs
                if not weak and not member.startswith('compiler_builtins')]
        if not ours:
            where = ', '.join(sorted({f'{member}{" (weak)" if weak else ""}' for member, weak in defs}))
            problems.append(f'  {name} (declared in {Path(functions[name]).relative_to(INCLUDE.resolve())}) '
                            f'is defined only by {where}')
    if args.elf:
        for name, defs in sorted(definitions(args.arch, args.elf).items()):
            if ('libm_math' in name or 'compiler_builtins4math' in name or 'compiler_builtins..math..' in name
                    or name in functions and Path(functions[name]).name == 'math.h'
                    and any(weak for _, weak in defs)):
                problems.append(f'  forbidden linked math symbol {name} in {args.elf.name}')
    if problems:
        print(f'libc-symbol-audit: {args.arch} libc.a lacks Breenix definitions of functions '
              'its headers declare:', file=sys.stderr)
        print('\n'.join(problems), file=sys.stderr)
        return 1
    missing = sum(1 for name in functions if name not in defined)
    print(f'  libc-symbol-audit ({args.arch}): {len(functions)} declared functions, '
          f'{len(functions) - missing} defined by Breenix, {missing} not yet written')
    return 0


if __name__ == '__main__':
    sys.exit(main())
