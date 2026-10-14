#!/usr/bin/env python3
"""Fail when a global symbol has two strong definitions among the std link's inputs.

  libc-duplicate-check.py --archive libc.a --tool-dir DIR [--link-input ARCHIVE ...]

The std programs link with --allow-multiple-definition, so a second definition of a
symbol never fails the link: the first one in link order wins, and a leftover stub can
silently stand in for the function that replaced it. This check finds them first:

  - a global symbol defined strongly by two members of libc.a (two crates of the
    staticlib, which rustc never sees together), and
  - a C symbol (not a Rust-mangled one) defined strongly both by libc.a and by another
    input of the std link (--link-input: the rlibs of the std build), which would take
    the place of libc.a's.

Weak definitions (compiler_builtins' mem* and str*) are the linker's to override and
are not reported. Rust-mangled symbols are expected in both libc.a and the std rlibs:
each bundles its own core and alloc.
"""
import argparse
import re
from pathlib import Path
import subprocess
import sys

STRONG = set('TDBRC')
LINE = re.compile(r'^(?P<archive>.*?)\[(?P<member>[^\]]*)\]: (?P<name>\S+) (?P<type>\S)')


def strong_definitions(nm, archive):
    """{symbol: {member, ...}} for every strong global definition in `archive`."""
    output = subprocess.run([str(nm), '-A', '--format=posix', '--defined-only', str(archive)],
                            capture_output=True, text=True, check=True).stdout
    found = {}
    for line in output.splitlines():
        match = LINE.match(line)
        if match and match['type'] in STRONG:
            found.setdefault(match['name'], set()).add(match['member'])
    return found


def mangled(name):
    return name.startswith(('_ZN', '_R'))


def duplicates(nm, archive, link_inputs):
    """Every duplicate, as (symbol, [where it is defined])."""
    libc = strong_definitions(nm, archive)
    found = [(name, sorted(f'{archive.name}[{m}]' for m in members))
             for name, members in libc.items() if len(members) > 1]
    for other in link_inputs:
        for name, members in strong_definitions(nm, other).items():
            if name in libc and not mangled(name):
                found.append((name, sorted(f'{archive.name}[{m}]' for m in libc[name])
                              + sorted(f'{other.name}[{m}]' for m in members)))
    return sorted(found)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--tool-dir', type=Path, required=True)
    parser.add_argument('--link-input', type=Path, action='append', default=[])
    args = parser.parse_args()
    found = duplicates(args.tool_dir / 'llvm-nm', args.archive, args.link_input)
    for name, places in found:
        print(f'  ERROR: {name} is defined more than once: {", ".join(places)}', file=sys.stderr)
    if found:
        print(f'  ERROR: {len(found)} duplicate definition(s) in the std link inputs; '
              '--allow-multiple-definition would keep whichever links first', file=sys.stderr)
        return 1
    print(f'  libc duplicate check: no symbol defined twice ({args.archive.name}, '
          f'{len(args.link_input)} other link input(s))')
    return 0


if __name__ == '__main__':
    sys.exit(main())
