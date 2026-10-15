#!/usr/bin/env python3
"""Shared pre-build policy for effort commits; ARM64 QEMU is always allowed."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout.strip()


def refresh_origin(root):
    # Bound both a dead network and SSH's credential prompts; never wait for a boot slot.
    env = dict(os.environ, GIT_TERMINAL_PROMPT='0',
               GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=3')
    with subprocess.Popen(['git', '-C', str(root), 'fetch', '--quiet', '--no-tags',
                           '--no-auto-gc', '--prune', 'origin',
                           '+refs/heads/*:refs/remotes/origin/*'], env=env,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                          start_new_session=True) as process:
        try:
            return process.wait(timeout=5) == 0
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            return False


def check(root, commit, target, catchup):
    sha = git(root, 'rev-parse', '--verify', commit + '^{commit}')
    if target == 'arm64-qemu':
        return sha
    if not refresh_origin(root):
        print('Cannot read origin; checking local remote-tracking branches only.', file=sys.stderr)
    branches = git(root, 'for-each-ref', '--contains=' + sha,
                   '--format=%(refname:strip=3)', 'refs/remotes/origin/').splitlines()
    # Main and regression/probe branches take precedence even if an effort merged them.
    if any(b == 'main' or b.startswith(('fix/', 'scratch/')) for b in branches):
        return sha
    with (root / 'docs/efforts/path.json').open() as source:
        ids = {effort['id'] for effort in json.load(source)['efforts']}
    efforts = sorted(b for b in branches if '/' in b and b.split('/', 1)[0] in ids)
    if efforts and not catchup:
        raise ValueError('Refusing ' + target + ' for origin/' + efforts[0] + ': effort work runs on '
                         'ARM64 QEMU until its catch-up; use --catchup (breenix-runs) or '
                         'BREENIX_CATCHUP=1 (run.sh) for the catch-up.')
    return sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--commit', default='HEAD')
    parser.add_argument('--target', choices=('x86', 'parallels', 'vmware', 'arm64-qemu'), required=True)
    parser.add_argument('--catchup', action='store_true')
    args = parser.parse_args()
    try:
        check(args.repo, args.commit, args.target, args.catchup)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
