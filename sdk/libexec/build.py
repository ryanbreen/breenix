#!/usr/bin/env python3
"""Build the current Breenix SDK project and print its executable path last."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

ARCHES = ('aarch64', 'x86_64')


def sdk_root():
    return Path(__file__).resolve().parents[3]


def project_config(project):
    # This SDK's manifest has exactly two flat TOML string values. Avoid tomllib,
    # which is unavailable on the supported Python 3.9 hosts.
    config = {}
    for line in (project / 'breenix.toml').read_text().splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        match = re.fullmatch(r'\s*(name|kind)\s*=\s*([\"\'])([A-Za-z0-9_-]+)\2\s*(?:#.*)?', line)
        if not match or match[1] in config:
            raise ValueError('breenix.toml requires one name and one kind string')
        config[match[1]] = match[3]
    if set(config) != {'name', 'kind'} or not re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]*', config['name']):
        raise ValueError('breenix.toml requires a valid project name and kind')
    if config['kind'] not in ('c', 'rust', 'rust-no-std'):
        raise ValueError('unknown project kind: {}'.format(config['kind']))
    return config


def build_c(project, root, arch, output):
    output.unlink(missing_ok=True)
    sources = sorted((project / 'src').glob('*.c'))
    if not sources:
        raise ValueError('the C project has no src/*.c files')
    command = [str(root / 'bin' / (arch + '-breenix-cc')),
               '-std=c17', '-O2', '-Wall', '-Wextra', *map(str, sources), '-o', str(output)]
    result = subprocess.run(command, cwd=project)
    if result.returncode:
        output.unlink(missing_ok=True)
        return result.returncode
    if not output.is_file():
        raise ValueError('the C driver succeeded without producing an executable')
    return 0


def build_rust(project, root, arch, name, output):
    command = [str(root / 'bin' / (arch + '-breenix-cargo')), 'build', '--release', '--offline',
               '--message-format=json-render-diagnostics']
    executable = None
    with subprocess.Popen(command, cwd=project, stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                print(line, end='', file=sys.stderr)
                continue
            if message.get('reason') == 'compiler-message':
                rendered = message.get('message', {}).get('rendered')
                if rendered:
                    print(rendered, end='', file=sys.stderr)
            if (message.get('reason') == 'compiler-artifact'
                    and message.get('target', {}).get('name') == name
                    and 'bin' in message.get('target', {}).get('kind', [])
                    and message.get('executable')):
                executable = Path(message['executable'])
        status = process.wait()
    if status:
        return status
    if executable is None:
        raise ValueError('cargo produced no executable artifact for {}'.format(name))
    if not executable.is_absolute():
        executable = project / executable
    if executable.resolve() != output.resolve():
        shutil.copy2(executable, output)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=ARCHES)
    parser.add_argument('--version', action='store_true')
    args = parser.parse_args()
    try:
        root = sdk_root()
        if args.version:
            stamp = (root / 'VERSION').read_text().split()
            print('{} {} build {}'.format(stamp[0], stamp[1], stamp[2]))
            return 0
        if not args.arch:
            parser.error('--arch is required')
        project = Path.cwd()
        config = project_config(project)
        output = project / 'build' / args.arch / config['name']
        output.parent.mkdir(parents=True, exist_ok=True)
        if config['kind'] == 'c':
            status = build_c(project, root, args.arch, output)
        else:
            status = build_rust(project, root, args.arch, config['name'], output)
        if status:
            return status
        print(output)
    except (OSError, ValueError) as error:
        print('breenix-build: {}'.format(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
