#!/usr/bin/env python3
"""Push a built desktop to a Mac; run under deploy-lock.sh run breenix-gus-mac."""
import json
from datetime import datetime, timezone
import re
import os
from pathlib import Path, PurePosixPath
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
DIGEST = '''import hashlib, pathlib, sys
p = pathlib.Path(sys.argv[1]).expanduser()
def file_hash(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1048576), b''): h.update(block)
    return h.hexdigest()
if p.is_dir():
    h = hashlib.sha256()
    for item in sorted(p.rglob('*')):
        if item.is_symlink(): kind, value = 'link', str(item.readlink())
        elif item.is_file(): kind, value = 'file', file_hash(item)
        elif item.is_dir(): kind, value = 'dir', ''
        else: raise ValueError(str(item))
        for field in (kind, item.relative_to(p).as_posix(), value): h.update(field.encode() + b'\\0')
    print(h.hexdigest())
else: print(file_hash(p))
'''


SWAP = """import pathlib, re, shutil, sys
home = pathlib.Path.home()
current, previous = home/'breenix', home/'breenix.prev'
for path in (current, previous):
    if not path.exists() and not path.is_symlink(): continue
    marker, git = path/'.deployed-commit', path/'.git'
    if (path.is_symlink() or not path.is_dir() or git.exists() or git.is_symlink()
        or marker.is_symlink() or not marker.is_file()
        or not re.fullmatch('[0-9a-f]{40}\\n?', marker.read_text())):
        sys.exit('refuse to replace non-deployment: ' + path.name)
if previous.exists(): shutil.rmtree(previous)
if current.exists(): current.rename(previous)
(home/'breenix.incoming').rename(current)
"""


def main():
    args = sys.argv[1:]
    push_only = '--push-only' in args
    if push_only:
        args.remove('--push-only')
    if len(args) != 1 or args[0].startswith('-') or '/' in args[0] or args[0] in ('.', '..'):
        sys.exit('usage: deploy-mac.sh [--push-only] HOST')
    host = args[0]
    os.chdir(ROOT)
    if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal']):
        sys.exit('deploy-mac: dirty tree')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    subprocess.run([str(ROOT / 'scripts/boot-interactive.sh'), '--mode', 'desktop', '--gpu-gl', '--build-only'], check=True)
    manifest = subprocess.check_output(['python3', str(ROOT / 'scripts/boot-artifacts.py'), '--mode', 'desktop', '--gpu-gl', '--json'], text=True)
    items = json.loads(manifest)

    def ssh(command, **kwargs):
        return subprocess.run(['ssh', host, command], check=True, **kwargs)

    def remote_path(item):
        if item['destination'] == 'QEMU-GL.app':
            return str(PurePosixPath('~') / Path(item['path']).relative_to(Path.home()).as_posix())
        dest = PurePosixPath(item['destination'])
        if dest.is_absolute() or '..' in dest.parts:
            raise ValueError('unsafe artifact destination')
        return str(PurePosixPath('~/breenix.incoming') / dest)

    def shell_path(path):
        return '"$HOME"/' + shlex.quote(path.removeprefix('~/'))

    def digest(path):
        command = 'python3 -c ' + shlex.quote(DIGEST) + ' ' + shell_path(path)
        result = subprocess.run(['ssh', host, command], text=True, capture_output=True)
        return result.stdout.strip() if result.returncode == 0 else None

    ssh('rm -rf "$HOME/breenix.incoming" && mkdir -p "$HOME/breenix.incoming"')
    archive = subprocess.Popen(['git', 'archive', 'HEAD'], stdout=subprocess.PIPE)
    try:
        ssh('tar -xf - -C "$HOME/breenix.incoming"', stdin=archive.stdout)
    finally:
        archive.stdout.close()
        status = archive.wait()
    if status:
        raise subprocess.CalledProcessError(status, 'git archive')
    for item in items:
        path = remote_path(item)
        if item['destination'] == 'QEMU-GL.app' and digest(path) == item['sha256']:
            continue
        if item['destination'] == 'QEMU-GL.app':
            ssh('rm -rf ' + shell_path(path))
        ssh('mkdir -p ' + shell_path(str(PurePosixPath(path).parent)))
        source = item['path'] + ('/' if item['destination'] == 'QEMU-GL.app' else '')
        subprocess.run(['rsync', '-a', source, host + ':' + path], check=True)
    for item in items:
        if digest(remote_path(item)) != item['sha256']:
            raise ValueError('remote artifact hash mismatch: ' + item['destination'])
    data = json.dumps(items, indent=2) + '\n'
    writer = "import pathlib,sys; p=pathlib.Path.home()/'breenix.incoming'; (p/'.deployed-commit').write_text(sys.argv[1]+'\\n'); (p/'.deployed-artifacts.json').write_text(sys.stdin.read())"
    ssh('python3 -c ' + shlex.quote(writer) + ' ' + shlex.quote(commit), input=data, text=True)
    ssh('python3 -c ' + shlex.quote(SWAP))
    if push_only:
        return
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    evidence = Path.home() / 'Library/Caches/breenix/deploy-mac' / host / stamp
    evidence.mkdir(parents=True)
    print(f'deploy-mac: evidence: {evidence}', flush=True)
    remote_dir = f'~/breenix-boots/{stamp}'
    command = ('cd "$HOME/breenix" && scripts/boot-interactive.sh --mode desktop --gpu-gl '
               '--no-build --no-display --until-desktop-ready --serial-log '
               + shell_path(remote_dir + '/serial.txt'))
    boot = subprocess.run(['ssh', '-T', host, command], stdin=subprocess.DEVNULL)
    copied = True
    for name in ('serial.txt', 'host-renderer.json'):
        result = subprocess.run(['rsync', '-a', host + ':' + remote_dir + '/' + name,
                                 str(evidence / name)])
        copied = copied and result.returncode == 0
    marker = subprocess.run(['ssh', '-T', host, 'cat "$HOME/breenix/.deployed-commit"'],
                            stdin=subprocess.DEVNULL, text=True, capture_output=True)
    if not copied:
        raise ValueError('could not copy boot evidence')
    judgment = subprocess.run(['python3', str(ROOT / 'scripts/check-host-renderer.py'),
                               str(evidence / 'host-renderer.json'), '--serial', str(evidence / 'serial.txt')])
    lines = re.findall(r'^==> Commit: (.*)$', (evidence / 'serial.txt').read_text(errors='replace'), re.MULTILINE)
    if marker.returncode or marker.stdout.strip() != commit or lines != [commit + ' deployed=1']:
        raise ValueError('boot commit does not match .deployed-commit')
    if boot.returncode or judgment.returncode:
        raise ValueError(f'boot exit {boot.returncode}, local renderer judgment exit {judgment.returncode}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f'deploy-mac: {error}')
