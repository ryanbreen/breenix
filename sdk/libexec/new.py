#!/usr/bin/env python3
"""Create a standalone project from the packaged Breenix templates."""
import argparse
import json
from pathlib import Path
import re
import sys

TEMPLATES = ('c', 'rust', 'rust-no-std')


def sdk_root():
    return Path(__file__).resolve().parents[3]


def version():
    stamp = (sdk_root() / 'VERSION').read_text().split()
    print('{} {} new {}'.format(stamp[0], stamp[1], stamp[2]))


def create(kind, destination):
    target = Path(destination).absolute()
    name = target.name
    if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]*', name):
        raise ValueError('project name must start with a letter and contain only letters, digits, - or _')
    # Refuse symlinks as well as occupied directories, before writing anything.
    if target.is_symlink() or (target.exists() and (not target.is_dir() or any(target.iterdir()))):
        raise ValueError('destination must be an empty directory or a new path')
    template = sdk_root() / 'share' / 'breenix-sdk' / 'templates' / kind
    files = sorted(p for p in template.rglob('*') if p.is_file())
    if not files:
        raise ValueError('the SDK is missing the {} template'.format(kind))
    contents = [(p.relative_to(template), p.read_text().replace('{{name}}', name)) for p in files]
    target.mkdir(parents=True, exist_ok=True)
    for relative, text in contents:
        output = target / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        with output.open('x') as stream:
            stream.write(text)
    with (target / 'breenix.toml').open('x') as stream:
        stream.write('name = {}\nkind = {}\n'.format(json.dumps(name), json.dumps(kind)))
    print('Created {} project {}'.format(kind, name))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', action='store_true')
    parser.add_argument('--list', action='store_true', help='list the packaged templates')
    parser.add_argument('template', nargs='?', choices=TEMPLATES)
    parser.add_argument('directory', nargs='?')
    args = parser.parse_args()
    try:
        if args.version:
            version()
        elif args.list:
            print(' '.join(TEMPLATES))
        elif args.template and args.directory:
            create(args.template, args.directory)
        else:
            parser.error('provide TEMPLATE DIRECTORY, --list or --version')
    except (OSError, ValueError) as error:
        print('breenix-new: {}'.format(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
