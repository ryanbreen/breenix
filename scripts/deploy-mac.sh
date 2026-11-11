#!/usr/bin/env python3
"""Push a built desktop to a Mac; run under deploy-lock.sh run breenix-gus-mac."""
import json
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


def main():
    if len(sys.argv) != 2 or sys.argv[1].startswith('-'):
        sys.exit('usage: deploy-mac.sh HOST')
    host = sys.argv[1]
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
        subprocess.run(['rsync', '-a', item['path'], host + ':' + path], check=True)
    for item in items:
        if digest(remote_path(item)) != item['sha256']:
            raise ValueError('remote artifact hash mismatch: ' + item['destination'])
    data = json.dumps(items, indent=2) + '\n'
    writer = "import pathlib,sys; p=pathlib.Path.home()/'breenix.incoming'; (p/'.deployed-commit').write_text(sys.argv[1]+'\\n'); (p/'.deployed-artifacts.json').write_text(sys.stdin.read())"
    ssh('python3 -c ' + shlex.quote(writer) + ' ' + shlex.quote(commit), input=data, text=True)
    ssh('rm -rf "$HOME/breenix.prev" && if [ -e "$HOME/breenix" ]; then mv "$HOME/breenix" "$HOME/breenix.prev"; fi && mv "$HOME/breenix.incoming" "$HOME/breenix"')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f'deploy-mac: {error}')
