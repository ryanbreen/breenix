#!/usr/bin/env python3
"""Verify the retained K5 sysroot and K6 linker capture against the repository pins."""

import json
import os
from pathlib import Path
import sys

from rustc_port import K5, sha256

K6 = K5.with_name('k6')
LINKS = ('bas', 'bld', 'bar', 'bnm', 'bcc', 'bmake', 'panic-abort', 'catch-unwind')


def read_json(path):
    try:
        return json.loads(path.read_text(encoding='utf-8'))
    except ValueError as error:
        raise ValueError(f'{path}: {error}') from error


def verify(fixture):
    if not fixture.is_dir():
        raise ValueError('retained fixture directory is missing')

    k5 = read_json(K5 / 'x86_64.json')
    k6 = read_json(K6 / 'x86_64.json')
    control = read_json(K6 / 'expect.json')['control']
    links = k6['links']
    if links.keys() != set(LINKS):
        raise ValueError('K6 build record must pin the six tool links and both probe links')

    pins = {}
    for name, digest in [
        *((f'sysroot/{name}', digest) for name, digest in k5['sysroot']['files_sha256'].items()),
        *((name, entry['sha256']) for name, entry in k6['inputs'].items()),
        *((f'links/{name}/reference.elf', link['reference_sha256']) for name, link in links.items()),
        (control['archive'], control['sha256']),
    ]:
        if pins.setdefault(name, digest) != digest:
            raise ValueError(f'{name}: conflicting SHA-256 pins in the build records')
    if len(pins) != 89:
        raise ValueError(f'expected 89 distinct pinned files, found {len(pins)}')

    for name, expected in pins.items():
        actual = sha256(fixture / name)
        if actual != expected:
            raise ValueError(f'{name}: SHA-256 differs (expected {expected}, got {actual})')
    for name, link in links.items():
        relative = f'links/{name}/argv.json'
        if read_json(fixture / relative) != link['argv']:
            raise ValueError(f'{relative}: argv differs from the K6 build record')
    print(f'BREENIX_K6_FIXTURE={fixture}: verified {len(pins)} SHA-256 digests and {len(links)} argv records')


if __name__ == '__main__':
    default = Path.home() / 'Library/Caches/breenix-e25/k6/fixture'
    fixture = Path(os.environ.get('BREENIX_K6_FIXTURE', default)).expanduser().resolve()
    try:
        verify(fixture)
    except (OSError, ValueError) as error:
        sys.exit(f'BREENIX_K6_FIXTURE={fixture}: {error}')
