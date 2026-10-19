#!/usr/bin/env python3
"""Package libbreenix as a cargo directory source for the SDK (stage 40, RUSTAPP).

    rust-vendor.py SOURCE VENDOR

Copies SOURCE (libs/libbreenix) to VENDOR/libbreenix-<version>/ with a normalized
manifest: the `suite` feature, its optional `libgfx` path dependency and the module
behind it are dropped, and so is the crate's own `[workspace]` table, so the package
resolves offline from the SDK alone. Every other dependency must be an in-tree path
crate (gpu-abi, the syscall 416 layouts libbreenix shares with the kernel): each is
vendored the same way as VENDOR/<name>-<version>/, with its own path dependencies, and
the depending manifest names it by that exact version instead of its path. A registry
dependency stops the build. Writes the `.cargo-checksum.json` cargo needs to use each
directory as a source. Python 3.9 standard library only.
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


def package_field(manifest, field):
    """A `[package]` string field of a manifest."""
    for header, lines in sections(manifest):
        if header == '[package]':
            for line in lines:
                match = re.match(r'^\s*' + field + r'\s*=\s*"([^"]+)"', line)
                if match:
                    return match.group(1)
    raise SystemExit(f'rust-vendor: a manifest has no [package] {field}')


def normalize_manifest(text, source, vendor, dropped):
    """The manifest with `[workspace]` and comments removed, `dropped` features and
    dependencies removed, and each path dependency vendored and named by version."""
    kept = []
    for header, lines in sections(text):
        if header == '[workspace]':
            continue
        lines = [l for l in lines if not l.lstrip().startswith('#')]
        if header == '[features]':
            lines = [l for l in lines if l.split('=')[0].strip() not in dropped]
        if header == '[dependencies]':
            deps = []
            for line in lines:
                if '=' not in line:
                    continue
                name = line.split('=')[0].strip()
                if name in dropped:
                    continue
                path = re.search(r'\bpath\s*=\s*"([^"]+)"', line)
                if not path:
                    raise SystemExit(f'rust-vendor: {name} is not an in-tree path crate, so the SDK cannot vendor it')
                version = vendor_crate(source / path.group(1), vendor)
                deps.append(line[:path.start()] + f'version = "={version}"' + line[path.end():])
            lines = deps
        while lines and not lines[-1].strip():
            lines.pop()
        if not lines and header == '[dependencies]':
            continue
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


def vendor_crate(source, vendor, root=False):
    """Copy the crate at `source` to VENDOR/<name>-<version>/; returns its version. The
    root crate (libbreenix) loses the dropped features, dependencies and files."""
    source = source.resolve()
    manifest = (source / 'Cargo.toml').read_text()
    name, version = package_field(manifest, 'name'), package_field(manifest, 'version')
    dest = vendor / f'{name}-{version}'
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    dropped = (DROPPED_FEATURES | DROPPED_DEPENDENCIES) if root else set()
    files = sorted(p for p in (source / 'src').rglob('*') if p.is_file())
    for path in files:
        rel = path.relative_to(source).as_posix()
        if root and rel in DROPPED_FILES:
            continue
        target = dest / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        data = path.read_bytes()
        if root and rel == 'src/lib.rs':
            data = strip_dropped_modules(data.decode()).encode()
        target.write_bytes(data)
    (dest / 'Cargo.toml').write_text(normalize_manifest(manifest, source, vendor, dropped))
    checksums = {}
    for path in sorted(p for p in dest.rglob('*') if p.is_file()):
        checksums[path.relative_to(dest).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    # A directory source with no .crate file has no package checksum.
    (dest / '.cargo-checksum.json').write_text(json.dumps({'files': checksums, 'package': None},
                                                          sort_keys=True, separators=(',', ':')))
    return version


def main():
    if len(sys.argv) != 3:
        raise SystemExit('usage: rust-vendor.py SOURCE VENDOR')
    print(vendor_crate(Path(sys.argv[1]), Path(sys.argv[2]), root=True))


if __name__ == '__main__':
    main()
