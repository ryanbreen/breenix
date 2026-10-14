#!/usr/bin/env bash
# Materialize and validate the pinned Breenix Rust library, independently of rustc.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 - "$ROOT" "$@" <<'PY'
import os
import hashlib
import json
import shutil
from pathlib import Path
import re
import subprocess
import sys
import tomllib

root = Path(sys.argv[1])
pin = tomllib.loads((root / 'rust-fork.toml').read_text())

def git(where, *args):
    return subprocess.check_output(['git', '-C', str(where), *args], text=True).strip()

def run(where, *args):
    subprocess.run(['git', '-C', str(where), *args], check=True)

def fingerprint(directory):
    digest = hashlib.sha256()
    for path in sorted(directory.rglob('*')):
        if path.is_file() and path.name != '.rust-fork-snapshot.json':
            digest.update(str(path.relative_to(directory)).encode() + b'\0' + path.read_bytes() + b'\0')
    return digest.hexdigest()

def archive(source, commit, destination, *paths, prefix=''):
    command = ['git', '-C', str(source), 'archive', '--format=tar']
    if prefix:
        command += ['--prefix=' + prefix]
    command += [commit, *paths]
    with subprocess.Popen(command, stdout=subprocess.PIPE) as producer:
        result = subprocess.run(['tar', '-xf', '-', '-C', str(destination)], stdin=producer.stdout)
        producer.stdout.close()
        if producer.wait() != 0 or result.returncode != 0:
            raise RuntimeError('failed to archive the pinned Rust library')

try:
    if not re.fullmatch(r'[0-9a-f]{40}', pin['commit']):
        raise RuntimeError('fork pin must be a full commit SHA')
    command = sys.argv[2] if len(sys.argv) > 2 else 'verify'
    checkout = root / 'rust-fork'
    if command == 'snapshot':
        if len(sys.argv) != 5:
            raise RuntimeError('usage: rust-fork.sh snapshot SOURCE DESTINATION')
        source, destination = map(Path, sys.argv[3:])
        run(source, 'fetch', 'origin', pin['branch'])
        run(source, 'fetch', 'origin', pin['commit'])
        git(source, 'cat-file', '-e', pin['commit'] + '^{commit}')
        if destination.exists():
            shutil.rmtree(destination)
        destination.mkdir(parents=True)
        archive(source, pin['commit'], destination, 'library')
        for row in git(source, 'ls-tree', '-r', pin['commit'], 'library').splitlines():
            entry, path = row.split('\t', 1)
            if entry.startswith('160000 '):
                archive(source / path, entry.split()[2], destination, prefix=path + '/')
        metadata = dict(commit=pin['commit'], rust=git(source, 'show', pin['commit'] + ':src/version'), digest=fingerprint(destination))
        (destination / '.rust-fork-snapshot.json').write_text(json.dumps(metadata) + '\n')
        print('Archived Breenix Rust fork ' + pin['commit'])
        sys.exit(0)
    if command == 'sync':
        # Resolve a pre-existing shared link before replacing it. Never change its target.
        source = os.environ.get('BREENIX_RUST_FORK_SOURCE')
        if source is None and checkout.exists():
            source = str(checkout.resolve())
        if source is None:
            for row in git(root, 'worktree', 'list', '--porcelain').splitlines():
                if row.startswith('worktree '):
                    candidate = Path(row[9:]) / 'rust-fork'
                    if candidate.exists() and candidate.resolve() != checkout:
                        source = str(candidate.resolve())
                        break
        if source is None:
            raise RuntimeError('set BREENIX_RUST_FORK_SOURCE to the canonical Rust clone')
        source = Path(source).resolve()
        run(source, 'fetch', 'origin', pin['branch'])
        run(source, 'fetch', 'origin', pin['commit'])
        git(source, 'cat-file', '-e', pin['commit'] + '^{commit}')
        if checkout.is_symlink():
            checkout.unlink()
        if not checkout.exists():
            args = sys.argv[3:]
            if args and (len(args) != 2 or args[0] != '--branch'):
                raise RuntimeError('usage: rust-fork.sh sync [--branch rust-std/<lane>]')
            branch = ['-b', args[1]] if args else ['--detach']
            run(source, 'worktree', 'add', '--no-checkout', *branch, str(checkout), pin['commit'])
            run(checkout, 'sparse-checkout', 'set', 'library')
            run(checkout, 'checkout')
        else:
            if git(checkout, 'status', '--porcelain'):
                raise RuntimeError('refusing to replace a dirty fork worktree')
            if git(checkout, 'rev-parse', 'HEAD') != pin['commit']:
                run(checkout, 'checkout', '--detach', pin['commit'])
        modules = [row.split('\t', 1)[1] for row in git(checkout, 'ls-tree', '-r', 'HEAD', 'library').splitlines() if row.startswith('160000 ')]
        if modules:
            run(checkout, 'submodule', 'update', '--init', '--depth', '1', *modules)
    elif command not in ('verify', 'check-reachable'):
        raise RuntimeError('usage: rust-fork.sh sync|verify|check-reachable')

    library = Path(os.environ.get('BREENIX_RUST_FORK_LIBRARY', str(checkout / 'library'))).resolve()
    fork = library.parent
    if command == 'check-reachable':
        run(fork, 'merge-base', '--is-ancestor', pin['commit'], 'origin/' + pin['branch'])
        print('Rust fork pin is reachable from origin/' + pin['branch'])
        sys.exit(0)
    if not (library / 'std/Cargo.toml').is_file():
        raise RuntimeError('Breenix Rust library is missing; run scripts/rust-fork.sh sync')
    snapshot = fork / '.rust-fork-snapshot.json'
    if snapshot.exists():
        metadata = json.loads(snapshot.read_text())
        head = metadata['commit']
        version = metadata['rust']
        if fingerprint(fork) != metadata['digest']:
            raise RuntimeError('Rust fork archive was modified after snapshot')
    else:
        head = git(fork, 'rev-parse', 'HEAD')
        version = git(fork, 'show', 'HEAD:src/version')
        if git(fork, 'status', '--porcelain', '--untracked-files=all'):
            raise RuntimeError('Rust fork worktree is dirty; commit the changes before building')
    if head != pin['commit']:
        if os.environ.get('BREENIX_RUST_FORK_DEV') != '1':
            raise RuntimeError('Rust fork HEAD does not match rust-fork.toml')
        run(fork, 'merge-base', '--is-ancestor', pin['commit'], head)
        print('UNPINNED Rust fork development build; boot is not acceptance evidence', file=sys.stderr)
    manifest = tomllib.loads((library / 'Cargo.toml').read_text())
    binding = (library / manifest['patch']['crates-io']['libc']['path']).resolve()
    if binding != (root / 'libs/libc').resolve():
        raise RuntimeError('Rust fork libc patch must resolve to this Breenix checkout; run sync')
    # Sparse worktrees omit src/version, so read the committed version through git.
    if version != pin['rust']:
        raise RuntimeError('Rust library version does not match rust-fork.toml')
    print('Breenix Rust fork ' + head)
except (RuntimeError, OSError, KeyError, subprocess.CalledProcessError) as error:
    print('rust-fork: ' + str(error), file=sys.stderr)
    sys.exit(1)
PY
