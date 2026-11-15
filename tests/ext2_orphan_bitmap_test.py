"""Run production orphan reclamation with the host storage fixture and strict warnings."""
from pathlib import Path
import os
import re
import shutil
import subprocess
import sys
import tempfile

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
HOST = ROOT / 'tools/ext2-host-tests'
CASES = [
    'ext2::reclaim_checks::orphan_tail_coalesces_bitmap_writes_within_the_transition_budget',
    'ext2::reclaim_checks::orphan_bitmap_batch_resumes_with_a_smaller_budget_and_a_new_allocation',
]


def main():
    with tempfile.TemporaryDirectory(prefix='ext2-orphan-bitmap-') as temp:
        target = Path(temp)
        (target / 'src').mkdir()
        for name in ['Cargo.toml', 'Cargo.lock']:
            shutil.copyfile(HOST / name, target / name)
        mount_path = ROOT / 'kernel/src/fs/ext2/mount_device.rs'
        mount = mount_path.read_text()
        # Reclaim does not use remounting. Omit this unrelated API from the
        # isolated host wrapper; reclaim.rs and block_group.rs remain intact.
        mount = mount.replace(item(mount, '    pub(super) fn remount_read_only('), '')
        wrapper = target / 'mount_device.rs'
        wrapper.write_text(mount)
        for source in (HOST / 'src').glob('*.rs'):
            def absolute(match):
                path = (source.parent / match[1]).resolve()
                if path == mount_path:
                    path = wrapper
                return f'#[path = "{path}"]'
            (target / 'src' / source.name).write_text(
                re.sub(r'#\[path = "([^"]+)"\]', absolute, source.read_text()))
        env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings')
        command = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        command += ['cargo', 'test', '--offline', '--manifest-path', str(target / 'Cargo.toml'), '--lib', '--']
        command += sys.argv[1:] or ['--exact', *CASES]
        subprocess.run(command, env=env, check=True)


if __name__ == '__main__':
    main()
