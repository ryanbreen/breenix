#!/usr/bin/env python3
"""Check the indexed archive contract from Solaris ar(3HEAD) and ELF gABI §5."""

import argparse
import os
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
LONG_NAME = 'long-archive-fixture-member.o'


def read_members(data):
    assert data[:8] == b'!<arch>\n'
    members = []
    pos = 8
    while pos < len(data):
        header = data[pos:pos + 60]
        assert header[16:48] == b'0           0     0     644     '
        assert header[58:] == b'`\n'
        size = int(header[48:58])
        end = pos + 60 + size
        assert end <= len(data)
        if size % 2:
            assert data[end:end + 1] == b'\n'
        members.append((pos, header[:16].rstrip(), data[pos + 60:end]))
        pos = end + size % 2
    return members


def index_is_deterministic(clang, bar, work):
    outputs = []
    for n in range(2):
        directory = work / str(n)
        directory.mkdir()
        sources = {
            LONG_NAME: b'.text\n.globl zeta\nzeta: .quad unresolved\nlocal_only: ret\n',
            'short.o': b'.text\n.weak alpha\nalpha: ret\n.comm shared,8,8\n',
        }
        for name, source in sources.items():
            subprocess.run([clang, '--target=x86_64-unknown-none-elf', '-x', 'assembler',
                            '-c', '-', '-o', name], cwd=directory, input=source, check=True)
            os.utime(directory / name, (1000 + n, 1000 + n))
            (directory / name).chmod(0o644 if n == 0 else 0o755)
        subprocess.run([bar, 'rcs', 'library.a', *sources], cwd=directory, check=True)
        output = (directory / 'library.a').read_bytes()
        subprocess.run([bar, 's', 'library.a'], cwd=directory, check=True)
        assert (directory / 'library.a').read_bytes() == output
        index, names, first, second = read_members(output)
        assert [m[1] for m in (index, names, first, second)] == [b'/', b'//', b'/0', b'short.o/']
        assert names[2] == LONG_NAME.encode() + b'/\n'
        count, *offsets = struct.unpack_from('>4I', index[2])
        assert count == 3
        entries = list(zip(index[2][16:].split(b'\0')[:-1], offsets))
        assert sorted(entries) == [(b'alpha', second[0]), (b'shared', second[0]), (b'zeta', first[0])]
        assert index[2].endswith(b'\0')
        assert first[2] == (directory / LONG_NAME).read_bytes()
        assert second[2] == (directory / 'short.o').read_bytes()
        outputs.append(output)
    assert outputs[0] == outputs[1], 'directory, mode and timestamps must not affect archive bytes'


def rebuild_archive_indexes(bar, work):
    original = (work / '0' / 'library.a').read_bytes()
    index, names, *_ = read_members(original)
    first_offset = index[0] + 60 + 4
    missing = original[:8] + original[names[0]:]
    corrupt = original[:first_offset] + b'\0' * 4 + original[first_offset + 4:]
    for label, damaged in [('missing', missing), ('corrupt', corrupt)]:
        assert damaged != original
        archive = work / f'{label}.a'
        archive.write_bytes(damaged)
        subprocess.run([bar, 's', archive], check=True)
        assert archive.read_bytes() == original, f'bar s did not rebuild the {label} index'
        print(f'PASS bar::rebuild_{label}_index ({work.name})', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--clang', default='clang')
    parser.add_argument('--host-arch', action='append', default=[])
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='bar-test-') as temp:
        for arch in args.host_arch or [None]:
            work = Path(temp) / (arch or 'native')
            work.mkdir()
            bar = work / 'bar'
            flags = ['-arch', arch] if arch else []
            subprocess.run([args.clang, *flags, '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-pedantic', '-fsanitize=address,undefined', '-fno-sanitize-recover=all',
                            ROOT / 'toolchain/bar/bar.c', ROOT / 'toolchain/common/ar.c',
                            ROOT / 'toolchain/common/elf_symbols.c', '-o', bar], check=True)
            index_is_deterministic(args.clang, bar, work)
            print(f'PASS bar::index_is_deterministic ({work.name})')
            rebuild_archive_indexes(bar, work)


if __name__ == '__main__':
    main()
