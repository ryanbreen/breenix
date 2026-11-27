#!/usr/bin/env python3
"""Select Cargo's executable and hosted dependencies during the K7 bootstrap."""

import os
from pathlib import Path
import sys


def main(args):
    env = os.environ.copy()
    cargo = env['K7_STAGE0_CARGO']
    if args and args[0] == 'build' and any(
            arg.endswith('/src/tools/cargo/Cargo.toml') for arg in args):
        args.extend(('--bin', 'cargo', '--features', 'all-static'))
        for key in ('RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER'):
            env.pop(key, None)
        env['RUSTC'] = env['K7_STAGE1_RUSTC']
        target = Path(args[args.index('--target') + 1])
        if target.name in ('x86_64-breenix-hosted.json', 'aarch64-breenix-hosted.json'):
            args.extend(('-Z', 'unstable-options', '--config', env['K7_CARGO_CONFIG'],
                         '--lockfile-path', env['K7_CARGO_LOCKFILE']))
            flags = ['--cfg=getrandom_backend="custom"']
            if target.name == 'aarch64-breenix-hosted.json':
                flags.append('-Crelocation-model=pic')
            if env.get('CARGO_ENCODED_RUSTFLAGS'):
                env['CARGO_ENCODED_RUSTFLAGS'] += '\x1f' + '\x1f'.join(flags)
            else:
                env['RUSTFLAGS'] = ' '.join([env.get('RUSTFLAGS', ''), *flags]).strip()
    os.execve(cargo, [cargo, *args], env)


if __name__ == '__main__':
    main(sys.argv[1:])
