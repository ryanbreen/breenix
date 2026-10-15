#!/usr/bin/env python3
"""Package libbreenix as a cargo directory source for the SDK (stage 40, RUSTAPP).

    rust-vendor.py SOURCE VENDOR

Copies SOURCE (libs/libbreenix) to VENDOR/libbreenix-<version>/ with a normalized
manifest: the `suite` feature, its optional `libgfx` path dependency and the module
behind it are dropped, and so is the crate's own `[workspace]` table, so the package
resolves offline from the SDK alone. Writes the `.cargo-checksum.json` cargo needs to
use the directory as a source. Python 3.9 standard library only.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import sys

# The suite feature is the in-tree effort runner; it needs std and libgfx by path.
DROPPED_FEATURES = {'suite'}
DROPPED_DEPENDENCIES = {'libgfx'}
DROPPED_FILES = {'src/suite.rs'}


def sections(text):
    """[(header or None, [lines])] in order; header is the `[table]` line."""
    out, header, lines = [], None, []
    for line in text.splitlines():
        if re.match(r'^\s*\[', line):
            out.append((header, lines))
            header, lines = line.strip(), []
        else:
            lines.append(line)
    out.append((header, lines))
    return out


def normalize_manifest(text):
    kept = []
    for header, lines in sections(text):
        if header == '[workspace]':
            continue
        if header == '[features]':
            lines = [l for l in lines if l.split('=')[0].strip() not in DROPPED_FEATURES]
        if header == '[dependencies]':
            names = {l.split('=')[0].strip() for l in lines if '=' in l and not l.lstrip().startswith('#')}
            if names - DROPPED_DEPENDENCIES:
                raise SystemExit('rust-vendor: libbreenix gained dependencies the SDK cannot vendor: '
                                 + ', '.join(sorted(names - DROPPED_DEPENDENCIES)))
            continue
        lines = [l for l in lines if not l.lstrip().startswith('#')]
        while lines and not lines[-1].strip():
            lines.pop()
        if header is None:
            kept.append('\n'.join(lines))
        else:
            kept.append('\n'.join([header] + lines))
    return '\n\n'.join(k for k in kept if k.strip()) + '\n'


def strip_dropped_modules(lib_rs):
    """Remove `#[cfg(feature = "<dropped>")]` and the item line after it."""
    lines, out, skip = lib_rs.splitlines(keepends=True), [], False
    for line in lines:
        if skip:
            skip = False
            continue
        match = re.match(r'^\s*#\[cfg\(feature = "([^"]+)"\)\]\s*$', line)
        if match and match.group(1) in DROPPED_FEATURES:
            skip = True
            continue
        out.append(line)
    return ''.join(out)


def main():
    if len(sys.argv) != 3:
        raise SystemExit('usage: rust-vendor.py SOURCE VENDOR')
    source, vendor = Path(sys.argv[1]), Path(sys.argv[2])
    manifest = (source / 'Cargo.toml').read_text()
    version = re.search(r'^version\s*=\s*"([^"]+)"', manifest, re.MULTILINE).group(1)
    dest = vendor / f'libbreenix-{version}'
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    files = sorted(p for p in (source / 'src').rglob('*') if p.is_file())
    for path in files:
        rel = path.relative_to(source).as_posix()
        if rel in DROPPED_FILES:
            continue
        target = dest / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        data = path.read_bytes()
        if rel == 'src/lib.rs':
            data = strip_dropped_modules(data.decode()).encode()
        target.write_bytes(data)
    (dest / 'Cargo.toml').write_text(normalize_manifest(manifest))
    checksums = {}
    for path in sorted(p for p in dest.rglob('*') if p.is_file()):
        checksums[path.relative_to(dest).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    # A directory source with no .crate file has no package checksum.
    (dest / '.cargo-checksum.json').write_text(json.dumps({'files': checksums, 'package': None},
                                                          sort_keys=True, separators=(',', ':')))
    print(version)


if __name__ == '__main__':
    main()
