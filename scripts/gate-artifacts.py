#!/usr/bin/env python3
"""Content-keyed x86 userspace/disks, published only after byte-identical clean verification."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import time
import importlib.util

spec = importlib.util.spec_from_file_location('gate_tree', Path(__file__).with_name('gate-tree.py'))
tree_cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tree_cache)


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as handle:
        while data := handle.read(1024 * 1024):
            result.update(data)
    return result.hexdigest()


def key(repo):
    result = hashlib.sha256(b'breenix-x86-userspace-v1\0')
    paths = subprocess.check_output(['git', '-C', str(repo), 'ls-files', '-z', '--',
        'userspace', 'libs', 'fonts', 'vendor/busybox', 'scripts', 'xtask',
        '.cargo', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'x86_64-breenix.json']).split(b'\0')
    for name in sorted(filter(None, paths)):
        path = repo / os.fsdecode(name)
        result.update(name + b'\0' + path.read_bytes() + b'\0')
    # The external fork is an input, not merely a symlink/commit assertion.
    library = Path(os.environ.get('BREENIX_RUST_FORK_LIBRARY', str(repo / 'rust-fork/library')))
    for path in sorted(library.rglob('*')):
        if path.is_file() and 'target' not in path.relative_to(library).parts:
            result.update(str(path.relative_to(library)).encode() + b'\0' + path.read_bytes() + b'\0')
    for command in (['rustc', '-Vv'], ['cargo', '-V'], ['mke2fs', '-V'], ['debugfs', '-V']):
        result.update(subprocess.check_output(command, stderr=subprocess.STDOUT))
    # Version strings alone cannot distinguish a locally patched compiler.
    rustc = Path(subprocess.check_output(['rustup', 'which', 'rustc'], text=True).strip())
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    for executable in [rustc] + sorted((sysroot / 'lib/rustlib').glob('*/bin/rust-lld')):
        result.update(digest(executable).encode())
    for variable in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'BREENIX_TRACE_DIAG_EARLY',
                     'BREENIX_BSSH_AUTORUN', 'BREENIX_WAIT_STRESS', 'RUSTUP_TOOLCHAIN'):
        result.update(variable.encode() + b'=' + os.environ.get(variable, '').encode() + b'\0')
    config = Path(os.environ['CARGO_HOME']) / 'config.toml'
    if config.exists():
        result.update(config.read_bytes())
    return result.hexdigest()


def canonical_ext2(path):
    """Normalize host clocks/UUID/hash seed/generations, never file bytes or topology.

    Gate images are clean ext2 filesystems. Their unmounted host bookkeeping is
    irrelevant to the guest but otherwise prevents byte-for-byte verification.
    Normalize every inode (including reserved ones) and backup superblock.
    """
    with path.open('r+b') as disk:
        disk.seek(1024)
        superblock = disk.read(1024)
        u16 = lambda offset: struct.unpack_from('<H', superblock, offset)[0]
        u32 = lambda offset: struct.unpack_from('<I', superblock, offset)[0]
        if u16(56) != 0xef53 or u32(96) & ~2:
            raise ValueError('canonicalization requires an ext2 filesystem with no unsupported incompat features')
        block = 1024 << u32(24)
        first, blocks, per_group, per_inode = u32(20), u32(4), u32(32), u32(40)
        groups = (blocks - first + per_group - 1) // per_group
        inode_size = u16(88) if u32(76) else 128
        disk.seek((first + 1) * block)
        descriptors = disk.read(groups * 32)
        for group in range(groups):
            table = struct.unpack_from('<I', descriptors, group * 32 + 8)[0]
            disk.seek(table * block)
            inodes = bytearray(disk.read(per_inode * inode_size))
            for index in range(per_inode):
                offset = index * inode_size
                # atime, ctime, mtime; dtime remains zero for a fresh image.
                struct.pack_into('<III', inodes, offset + 8, 946684800, 946684800, 946684800)
                struct.pack_into('<I', inodes, offset + 100, 0)  # generation
                if inode_size >= 160:
                    inodes[offset + 132:offset + 152] = bytes(20)  # extra time fields/crtime
            disk.seek(table * block)
            disk.write(inodes)
            position = 1024 if group == 0 else (first + group * per_group) * block
            disk.seek(position)
            backup = bytearray(disk.read(1024))
            if len(backup) == 1024 and struct.unpack_from('<H', backup, 56)[0] == 0xef53:
                for offset in (44, 48, 64, 264):  # mtime, wtime, lastcheck, mkfs_time
                    struct.pack_into('<I', backup, offset, 946684800)
                backup[104:120] = bytes.fromhex('7fabead6ba124c3b835be076230994d9')
                backup[236:252] = bytes(16)  # directory hash seed
                backup[136:200] = bytes(64)  # last mount path, host-specific
                disk.seek(position)
                disk.write(backup)


def checked(command, repo, log):
    environment = dict(os.environ, CARGO_BUILD_JOBS='6', BREENIX_USERSPACE_REMAP='1')
    with log.open('w') as output:
        status = subprocess.call(command, cwd=repo, env=environment, stdout=output, stderr=subprocess.STDOUT)
    if status:
        raise RuntimeError(f'{command[0]} failed ({status}); see {log}')
    # Match compile notices at the start of a line, even indented by build.sh.
    warnings = [line for line in log.read_text(errors='replace').splitlines()
                if line.lstrip().startswith(('warning:', 'warning[', 'error:', 'error['))
                and 'NEON' not in line and 'neon' not in line]
    if warnings:
        raise RuntimeError(f'build warnings/errors: {warnings[:5]}; see {log}')


def build(repo, logs, label):
    start = time.monotonic()
    print(f'[gate-phase] phase=userspace-build started={time.time():.3f} build={label}', flush=True)
    checked([str(repo / 'userspace/programs/build.sh')], repo, logs / f'{label}-userspace.log')
    tree_cache.phase('userspace-build', start)
    start = time.monotonic()
    print(f'[gate-phase] phase=disk-repack started={time.time():.3f} build={label}', flush=True)
    checked(['python3', str(repo / 'scripts/install-busybox.py'), 'x86_64'], repo, logs / f'{label}-busybox.log')
    checked(['cargo', 'run', '-p', 'xtask', '--', 'create-test-disk'], repo, logs / f'{label}-test-disk.log')
    checked([str(repo / 'scripts/create_ext2_disk.sh')], repo, logs / f'{label}-ext2.log')
    canonical_ext2(repo / 'testdata/ext2.img')
    shutil.copyfile(repo / 'testdata/ext2.img', repo / 'target/ext2.img')
    tree_cache.phase('disk-repack', start)


def artifacts(repo):
    return sorted((repo / 'userspace/programs').glob('*.elf')) + [repo / 'target/test_binaries.img', repo / 'testdata/ext2.img']


def inventory(directory):
    return {str(p.relative_to(directory)): digest(p) for p in artifacts(directory)}


def install(entry, repo, manifest):
    # Remove leftovers first; deletions in build.sh must retire stale binaries.
    for path in (repo / 'userspace/programs').glob('*.elf'):
        if str(path.relative_to(repo)) not in manifest:
            path.unlink()
    for name in manifest:
        target = repo / name
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.is_file() or digest(target) != manifest[name]:
            shutil.copyfile(entry / name, target)
    shutil.copyfile(repo / 'testdata/ext2.img', repo / 'target/ext2.img')


def main():
    repo, logs = map(Path, sys.argv[1:])
    if os.environ.get('BREENIX_GATE_FRESH') == '1' or 'BREENIX_GATE_CACHE_DIR' not in os.environ:
        build(repo, logs, 'uncached')
        return
    root = Path(os.environ['BREENIX_GATE_CACHE_DIR'])
    cache_key = key(repo)
    entry = root / 'artifacts' / cache_key
    wait = time.monotonic()
    with tree_cache.lease(root, 'artifacts-' + cache_key):
        tree_cache.phase('artifact-slot-wait', wait)
        start = time.monotonic()
        if (entry / 'verified.json').exists():
            manifest = json.loads((entry / 'verified.json').read_text())
            if not isinstance(manifest, dict) or not {'target/test_binaries.img', 'testdata/ext2.img'} <= manifest.keys() or not any(name.startswith('userspace/programs/') and name.endswith('.elf') for name in manifest):
                raise RuntimeError('invalid verified artifact inventory')
            if any(Path(name).is_absolute() or '..' in Path(name).parts for name in manifest):
                raise RuntimeError('invalid verified artifact path')
            if any(not (entry / name).is_file() or digest(entry / name) != checksum for name, checksum in manifest.items()):
                raise RuntimeError(f'corrupt artifact cache {cache_key}; refusing cached inputs')
            install(entry, repo, manifest)
            os.utime(entry, None)
            print(f'[gate-cache] HIT key={cache_key} clean-verified=true', flush=True)
            tree_cache.phase('userspace-cache', start)
            return
        print(f'[gate-cache] MISS key={cache_key}', flush=True)
        tree_cache.prune(root, protected=(repo, entry), reserve=1024**3)
        # Never leave a partly published key after a cancelled build.
        if entry.exists():
            shutil.rmtree(entry)
        entry.parent.mkdir(parents=True, exist_ok=True)
        entry.mkdir()
        staging = entry
        for elf in (repo / 'userspace/programs').glob('*.elf'):
            elf.unlink()
        build(repo, logs, 'candidate')
        manifest = inventory(repo)
        for name in manifest:
            target = staging / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(repo / name, target)
        # Same paths/flags, empty Cargo target directories, fresh ELFs/disks.
        # Cargo homes stay private to this supervisor, as required by #1130.
        for target in ('userspace/programs/target', 'libs/libbreenix-libc/target'):
            shutil.rmtree(repo / target, ignore_errors=True)
        for elf in (repo / 'userspace/programs').glob('*.elf'):
            elf.unlink()
        build(repo, logs, 'clean-verification')
        actual = inventory(repo)
        if actual != manifest:
            differences = sorted(name for name in manifest.keys() | actual.keys() if manifest.get(name) != actual.get(name))
            raise RuntimeError(f'cached candidate differs from clean build: {differences}')
        (staging / 'verified.json').write_text(json.dumps(manifest, sort_keys=True) + '\n')
        print(f'[gate-cache] VERIFIED key={cache_key} files={len(manifest)} byte-identical=true', flush=True)
        tree_cache.phase('cache-verification', start)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'GATE: FAIL ({error})', flush=True)
        sys.exit(1)
