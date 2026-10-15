#!/usr/bin/env python3
"""Shared pre-build policy for effort commits; ARM64 QEMU is always allowed.

A commit carries effort work when any commit in its history that origin/main lacks is also
in the history of an effort lane (`<effort id>/...`): a branch on origin, read live with
`git ls-remote`, or a local branch. Such a commit runs on x86-64, Parallels and VMware only
for the catch-up (--catchup or BREENIX_CATCHUP=1). When origin cannot be read the guard
refuses, because it cannot tell. On success it prints the full commit SHA.

A launcher that has already checked a commit exports BREENIX_TARGET_POLICY_CHECKED=<target>:<sha>;
nested launchers of the same target and commit (host-slot re-exec, the beast gate) skip the check.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

TIMEOUT_SECONDS = 15
TERM_GRACE_SECONDS = 3
TARGETS = ('x86', 'parallels', 'vmware', 'arm64-qemu')


class OriginUnavailable(Exception):
    pass


def git(root, *args):
    result = subprocess.run(['git', '-C', str(root), *args], text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode != 0:
        raise ValueError('git ' + ' '.join(args) + ' failed: ' + result.stderr.strip())
    return result.stdout.strip()


def origin(root, *args):
    """Run a network git command, bounded; SIGTERM first so git removes its own temporary files."""
    env = dict(os.environ, GIT_TERMINAL_PROMPT='0',
               GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=5')
    with subprocess.Popen(['git', '-C', str(root), *args], env=env, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          start_new_session=True) as process:
        try:
            out, err = process.communicate(timeout=TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=TERM_GRACE_SECONDS)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise OriginUnavailable('git ' + args[0] + ' took longer than '
                                    + str(TIMEOUT_SECONDS) + ' s')
        if process.returncode != 0:
            raise OriginUnavailable('git ' + args[0] + ' failed: ' + err.strip())
        return out


def fetch_objects(root, shas):
    # Objects only: no refs, no FETCH_HEAD, nothing pruned.
    origin(root, 'fetch', '--quiet', '--no-tags', '--no-write-fetch-head', '--no-auto-gc',
           '--no-recurse-submodules', 'origin', *shas)


def resolve(root, commit):
    result = subprocess.run(['git', '-C', str(root), 'rev-parse', '--verify', '--quiet',
                             commit + '^{commit}'], text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL)
    return result.stdout.strip() if result.returncode == 0 else None


def present(root, shas):
    result = subprocess.run(['git', '-C', str(root), 'cat-file', '--batch-check=%(objectname) %(objecttype)'],
                            input=''.join(sha + '\n' for sha in shas), text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True)
    return {line.split()[0] for line in result.stdout.splitlines() if line.endswith(' commit')}


def catchup_from_env():
    value = os.environ.get('BREENIX_CATCHUP', '').strip().lower()
    if value in ('', '0', 'false', 'no'):
        return False
    if value in ('1', 'true', 'yes'):
        return True
    raise ValueError('BREENIX_CATCHUP must be 1, true or yes for the catch-up, or 0, false, no or unset; got '
                     + repr(os.environ['BREENIX_CATCHUP']))


def already_checked(root, commit, target):
    stamp_target, _, stamp_sha = os.environ.get('BREENIX_TARGET_POLICY_CHECKED', '').partition(':')
    if stamp_target != target or not stamp_sha:
        return None
    sha = resolve(root, commit)
    return sha if sha is not None and sha == resolve(root, stamp_sha) else None


def check(root, commit, target, catchup):
    if target == 'arm64-qemu':
        sha = resolve(root, commit)
        if sha is None:
            raise ValueError('Cannot resolve commit ' + commit)
        return sha
    sha = already_checked(root, commit, target)
    if sha is not None:
        return sha
    try:
        heads = {}
        for line in origin(root, 'ls-remote', '--heads', 'origin').splitlines():
            tip, ref = line.split('\t')
            heads['origin/' + ref.removeprefix('refs/heads/')] = tip
        if 'origin/main' not in heads:
            raise OriginUnavailable('origin has no main branch')
        sha = resolve(root, commit)
        if sha is None:
            try:
                fetch_objects(root, [commit])
            except OriginUnavailable as error:
                raise ValueError('Cannot resolve commit ' + commit + ' locally or on origin: ' + str(error))
            sha = resolve(root, commit)
            if sha is None:
                raise ValueError('Cannot resolve commit ' + commit + ' locally or on origin')
        with (root / 'docs/efforts/path.json').open() as source:
            ids = {effort['id'] for effort in json.load(source)['efforts']}
        lanes = {name: tip for name, tip in heads.items()
                 if name.split('/')[1] in ids and name.count('/') >= 2}
        for line in git(root, 'for-each-ref', '--format=%(refname:strip=2) %(objectname)',
                        'refs/heads/').splitlines():
            name, tip = line.split(' ')
            if '/' in name and name.split('/', 1)[0] in ids:
                lanes[name] = tip
        main = heads['origin/main']
        wanted = sorted({main, *lanes.values()})
        missing = sorted(set(wanted) - present(root, wanted))
        if missing:
            fetch_objects(root, missing)
    except OriginUnavailable as error:
        raise ValueError('Refusing ' + target + ': cannot read origin (' + str(error) + '), so whether '
                         'this commit carries effort work is unknown. Retry once origin is reachable.')
    own = git(root, 'rev-list', sha, '--not', main).splitlines()
    if not own or not lanes:
        return sha
    by_tip = {}
    for name, tip in lanes.items():
        by_tip.setdefault(tip, []).append(name)
    reached = {}
    for line in git(root, 'log', '--format=%H %S', *sorted(by_tip), '--not', main).splitlines():
        found, source = line.split()
        reached[found] = source
    hits = [found for found in own if found in reached]
    if hits and not catchup:
        lane = sorted(by_tip[reached[hits[0]]])[0]
        raise ValueError('Refusing ' + target + ' for ' + sha[:12] + ': it carries commit '
                         + hits[0][:12] + ' of effort lane ' + lane + ', and effort work runs on '
                         'ARM64 QEMU until its catch-up; use --catchup (breenix-runs) or '
                         'BREENIX_CATCHUP=1 for the catch-up.')
    return sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, required=True)
    parser.add_argument('--commit', default='HEAD')
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--catchup', action='store_true')
    args = parser.parse_args()
    try:
        print(check(args.repo, args.commit, args.target, args.catchup or catchup_from_env()))
    except (OSError, ValueError, KeyError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
