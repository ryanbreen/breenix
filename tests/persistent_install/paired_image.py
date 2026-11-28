#!/usr/bin/env python3
"""Host side of the paired rehearsal (tests/persistent_install/paired_image.rs).

  paired_image.py inputs IMAGE STAGE      the inventory the paired deploy publishes for IMAGE:
                                          STAGE/inventory.tsv and STAGE/objects.json, by
                                          metal_live_flash.prepare_inventory
  paired_image.py objects STAGE OUT       the object set for the rows `paired have` did not
                                          print (stdin), by metal_live_flash.export_objects,
                                          unpacked into OUT as stage_objects unpacks it
  paired_image.py fixture IMAGE           write the committed fixture image (below)

The fixture image holds, in a few KiB, the parts of the real system root that the installer
has had to handle: a BusyBox hard-link group spanning /bin and /sbin, a second group, set-ID
su, relative and absolute symlinks, a symlink whose target the image lacks, a slow symlink,
two independent files with one SHA-256, an empty file, a name ending in byte 0xff linked to
its ASCII twin, and the Rust toolchain selection file. It is built with mke2fs and debugfs,
so no host file system has to accept its names or owners.
"""
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

TOOLS = Path(__file__).resolve().parents[2] / 'scripts/metal'
sys.path.insert(0, str(TOOLS))

import metal_live_flash  # noqa: E402
from metal_root import require_e2fsprogs  # noqa: E402

MTIME = 1795000000
# (path, kind, mode, uid, gid, content or target). A 'link' row names its group's first path.
FIXTURE = [
    (b'/bin', 'dir', 0o755, 0, 0, None),
    (b'/sbin', 'dir', 0o755, 0, 0, None),
    (b'/lib', 'dir', 0o755, 0, 0, None),
    (b'/usr', 'dir', 0o755, 0, 0, None),
    (b'/usr/bin', 'dir', 0o755, 0, 0, None),
    (b'/usr/lib', 'dir', 0o755, 0, 0, None),
    (b'/usr/lib/breenix', 'dir', 0o755, 0, 0, None),
    (b'/usr/libexec', 'dir', 0o755, 0, 0, None),
    (b'/usr/libexec/breenix', 'dir', 0o755, 0, 0, None),
    (b'/usr/share', 'dir', 0o755, 0, 0, None),
    (b'/usr/share/factory', 'dir', 0o755, 0, 0, None),
    (b'/usr/share/doc', 'dir', 0o755, 0, 0, None),
    (b'/bin/busybox', 'reg', 0o755, 0, 0, b'busybox applets\n' * 64),
    (b'/bin/cat', 'link', 0, 0, 0, b'/bin/busybox'),
    (b'/bin/ls', 'link', 0, 0, 0, b'/bin/busybox'),
    (b'/sbin/ifconfig', 'link', 0, 0, 0, b'/bin/busybox'),
    (b'/sbin/mount', 'link', 0, 0, 0, b'/bin/busybox'),
    (b'/usr/bin/env', 'link', 0, 0, 0, b'/bin/busybox'),
    (b'/bin/su', 'reg', 0o4755, 0, 0, b'clean-room su\n'),
    (b'/usr/bin/passwd', 'reg', 0o4755, 0, 0, b'clean-room passwd\n'),
    (b'/bin/sh', 'symlink', 0o777, 0, 0, b'bsh'),
    (b'/bin/bsh', 'reg', 0o755, 0, 0, b'breenix shell\n'),
    (b'/sbin/init', 'reg', 0o755, 0, 0, b'init\n'),
    (b'/sbin/persistent-install', 'reg', 0o755, 0, 0, b'installer\n'),
    (b'/sbin/reboot', 'reg', 0o755, 0, 0, b'reboot or poweroff\n'),
    (b'/sbin/poweroff', 'link', 0, 0, 0, b'/sbin/reboot'),
    (b'/lib/libc.so', 'reg', 0o755, 0, 0, b'clean-room libc\n' * 32),
    (b'/lib/ld-breenix.so', 'reg', 0o755, 0, 0, b'loader\n' * 16),
    (b'/usr/lib/libc.so', 'symlink', 0o777, 0, 0, b'/lib/libc.so'),
    (b'/usr/lib/libmissing.so', 'symlink', 0o777, 0, 0, b'../../opt/not-in-image/libmissing.so'),
    (b'/usr/lib/breenix/rust-current', 'reg', 0o644, 0, 0, b'rust-fixture-1\n'),
    (b'/usr/libexec/breenix/rust-std-child_test', 'reg', 0o755, 0, 0, b'child test\n'),
    (b'/usr/libexec/breenix/rust-std-child_test\xff', 'link', 0, 0, 0,
     b'/usr/libexec/breenix/rust-std-child_test'),
    (b'/usr/share/doc/a.txt', 'reg', 0o644, 0, 0, b'same bytes\n'),
    (b'/usr/share/doc/b.txt', 'reg', 0o644, 0, 0, b'same bytes\n'),
    (b'/usr/share/doc/empty', 'reg', 0o644, 0, 0, b''),
    (b'/usr/share/doc/slow-link', 'symlink', 0o777, 0, 0, b'a.txt/' + b'x' * 80),
    (b'/usr/share/factory/generation', 'reg', 0o644, 0, 0, b'fixture\n'),
]


def debugfs_name(path):
    # debugfs splits its arguments on blanks and reads them as C strings.
    if re.search(rb'[\s"\\\x00]', path):
        raise ValueError(f'fixture name debugfs cannot pass: {path!r}')
    return path


def fixture(image):
    mke2fs, debugfs = require_e2fsprogs('mke2fs', 'debugfs')
    image = Path(image)
    image.unlink(missing_ok=True)
    env = dict(os.environ, LC_ALL='C', TZ='UTC', E2FSPROGS_FAKE_TIME=str(MTIME))
    subprocess.run([mke2fs, '-q', '-F', '-t', 'ext2', '-b', '1024', '-N', '128', '-I', '256',
                    '-E', 'root_owner=0:0', str(image), '1024'], check=True, env=env)
    with tempfile.TemporaryDirectory(prefix='paired-fixture-') as scratch:
        commands = []
        links = {}
        for index, (path, kind, mode, uid, gid, value) in enumerate(FIXTURE):
            name = debugfs_name(path)
            if kind == 'dir':
                commands.append(b'mkdir ' + name)
            elif kind == 'reg':
                source = Path(scratch) / str(index)
                source.write_bytes(value)
                commands.append(b'write ' + str(source).encode() + b' ' + name)
            elif kind == 'symlink':
                commands.append(b'symlink ' + name + b' ' + debugfs_name(value))
            else:
                commands.append(b'ln ' + debugfs_name(value) + b' ' + name)
                links[value] = links.get(value, 1) + 1
                continue
            commands += [b'sif ' + name + b' ' + field for field in (
                b'mode 0%o' % ({'dir': 0o040000, 'reg': 0o100000, 'symlink': 0o120000}[kind] | mode),
                b'uid %d' % uid, b'gid %d' % gid, b'mtime @%d' % MTIME)]
        commands += [b'sif ' + first + b' links_count %d' % count for first, count in links.items()]
        for top in (b'/', b'/bin', b'/sbin', b'/lib', b'/usr'):
            commands.append(b'sif ' + top + b' mtime @%d' % MTIME)
        script = Path(scratch) / 'commands'
        script.write_bytes(b'\n'.join(commands) + b'\n')
        result = subprocess.run([debugfs, '-w', '-f', str(script), str(image)],
                                capture_output=True, env=env)
        errors = [line for line in result.stderr.splitlines()
                  if line and not line.startswith(b'debugfs ')]
        if result.returncode or errors:
            raise ValueError('debugfs could not build the fixture: '
                             + b'\n'.join(errors).decode(errors='replace'))
    fsck, = require_e2fsprogs('e2fsck')
    check = subprocess.run([fsck, '-fn', str(image)], capture_output=True, env=env)
    if check.returncode:
        raise ValueError('fixture image fails e2fsck: ' + check.stdout.decode(errors='replace'))


def inputs(image, stage):
    stage = Path(stage)
    (stage / 'inputs').mkdir(parents=True)
    (stage / 'inputs/ext2.img').symlink_to(Path(image).resolve())
    metal_live_flash.prepare_inventory(stage, stage / 'inputs')


def objects(stage, out):
    have = sys.stdin.read()
    # The same receipt check and unpacking as metal_live_flash.stage_objects.
    if any(not re.fullmatch(r'(?:[0-9a-f]{2})+', row) for row in have.splitlines()):
        raise ValueError('invalid paired have receipt')
    out = Path(out)
    packed = io.BytesIO()
    metal_live_flash.export_objects(Path(stage), set(have.splitlines()), packed)
    packed.seek(0)
    with tarfile.open(fileobj=packed, mode='r|') as archive:
        for member in archive:
            if not member.isfile() or not metal_live_flash.DIGEST.fullmatch(member.name):
                raise ValueError('invalid streamed object name or type')
            path = out / member.name
            with path.open('xb') as output, archive.extractfile(member) as source:
                shutil.copyfileobj(source, output)
            path.chmod(0o600)


def main():
    command, *words = sys.argv[1:]
    if command == 'inputs' and len(words) == 2:
        inputs(*words)
    elif command == 'objects' and len(words) == 2:
        objects(*words)
    elif command == 'fixture' and len(words) == 1:
        fixture(*words)
    else:
        raise SystemExit(__doc__)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f'paired_image.py: {error}')
