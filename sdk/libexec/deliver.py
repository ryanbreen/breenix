#!/usr/bin/env python3
"""Stage a Mac artifact or publish it through an ordinary transfer fixture boot."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys


def artifact_facts(path):
    digest = hashlib.sha256()
    size = 0
    if not path.is_file():
        raise ValueError('artifact must be a regular file')
    with path.open('rb') as stream:
        while chunk := stream.read(1 << 20):
            digest.update(chunk)
            size += len(chunk)
    if not size:
        raise ValueError('artifact must be nonempty')
    return dict(sha256=digest.hexdigest(), size=size)


def stage(args, facts):
    builder = args.repo / 'scripts/metal/metal-build-local'
    command = [sys.executable, str(builder), '--host', args.host, '--sha', args.sha,
               '--tools-commit', args.sha, '--mode', 'suite', '--suite', 'transfer-fixture,users',
               '--jobs', '4', '--transfer-artifact', str(args.artifact.resolve())]
    # Keep build diagnostics live. The existing hashed upload prints one
    # machine-readable prebuilt receipt as its final line.
    last = ''
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True) as child:
        for line in child.stdout:
            if line.strip():
                last = line
            print(line, end='', file=sys.stderr, flush=True)
        if child.wait():
            raise ValueError('Mac prebuilt builder or boot-host handoff failed')
    receipt = json.loads(last)
    if (receipt.get('commit') != args.sha or receipt.get('toolsCommit') != args.sha
            or receipt.get('transferInput') != facts
            or not re.fullmatch(r'\d{8}T\d{6}Z-metal', receipt.get('id', ''))):
        raise ValueError('boot-host receipt differs from the Mac artifact or commit')
    print(receipt['id'])
    return 0


def publish(args, facts):
    command = 'exec "$HOME/.local/share/breenix-metal/transfer-hostssh" ' + shlex.join(
        ('--boot', args.boot, '--sha', args.sha, '--wait', '12', '--dest=' + args.dest,
         '--mode', args.mode, '--artifact-sha', facts['sha256']))
    try:
        result = subprocess.run(['/usr/bin/ssh', '-T', '-o', 'BatchMode=yes',
                                 '-o', 'StrictHostKeyChecking=yes', '-o', 'ForwardAgent=no',
                                 '-o', 'ClearAllForwardings=yes', '-o', 'ConnectTimeout=3',
                                 args.host, 'sh -c ' + shlex.quote(command)],
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=58)
    except (OSError, subprocess.SubprocessError):
        result = None
    if result is not None:
        message = result.stdout.rstrip('\n')
        expected = {0: 'published', 1: 'not published', 2: 'unknown', 3: 'published; directory sync failed'}
        prefix = expected.get(result.returncode)
        if prefix and (message == prefix or message.startswith(prefix + '; ')) and '\n' not in message:
            print(message)
            return result.returncode
        if result.stderr:
            print(result.stderr.rstrip(), file=sys.stderr)
    print('unknown; no publication receipt from the boot host')
    return 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', action='store_true')
    sub = parser.add_subparsers(dest='action')
    prepare = sub.add_parser('stage', help='build and upload the hashed prebuilt fixture input')
    prepare.add_argument('--repo', type=Path, required=True)
    send = sub.add_parser('publish', help='publish through a leased transfer fixture already booting')
    send.add_argument('--boot', required=True)
    send.add_argument('--dest', required=True)
    send.add_argument('--mode', default='755')
    for action in (prepare, send):
        action.add_argument('--host', required=True, help='boot host SSH alias')
        action.add_argument('--sha', required=True, help='full fixture image and tools commit')
        action.add_argument('artifact', type=Path)
    args = parser.parse_args()
    if args.version:
        try:
            print((Path(__file__).resolve().parents[3] / 'VERSION').read_text().strip())
        except OSError as error:
            print(f'breenix-deliver: {error}', file=sys.stderr)
            return 1
        return 0
    if args.action is None:
        parser.error('choose stage or publish')
    if not re.fullmatch(r'[A-Za-z0-9_.@-]+', args.host) or args.host.startswith('-'):
        parser.error('--host requires a boot host SSH alias')
    if not re.fullmatch(r'[a-f0-9]{40}', args.sha):
        parser.error('--sha requires a full commit')
    if args.action == 'publish' and (not re.fullmatch(r'\d{8}T\d{6}Z-metal', args.boot)
                                     or not args.dest or not re.fullmatch(r'[0-7]{1,3}', args.mode)):
        parser.error('publish requires a metal boot ID, a destination and octal mode 000 through 777')
    try:
        facts = artifact_facts(args.artifact)
        return stage(args, facts) if args.action == 'stage' else publish(args, facts)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'breenix-deliver: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
