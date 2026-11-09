"""Check late aggregate consumers against the production boot capture (#2824)."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]

with tempfile.TemporaryDirectory(prefix='boot-ribbon-capture-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'Cargo.toml').write_text('\n'.join([
        '[package]', 'name="boot-ribbon-capture-check"', 'version="0.1.0"',
        'edition="2021"', '[dependencies]', 'spin="0.9.8"',
    ]))
    # Deliberately disjoint alternatives let the independent oracle use a simple
    # line search, without sharing the production marker matcher.
    (path / 'boot_ribbon.rs').write_text(
        '&["BOOT_MEMORY_READY", "BOOT_FS_READY|BOOT_STORAGE_READY", '
        '"BOOT_USERS_READY", "BOOT_NEVER_READY"]\n'
    )
    modules = f'''extern crate alloc;
pub mod console {{
    #[path="{ROOT}/kernel/src/console/status.rs"] pub mod status;
}}
pub mod graphics {{
    #[path="{ROOT}/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;
    #[path="{ROOT}/kernel/src/graphics/boot_ribbon.rs"] pub mod boot_ribbon;
}}
'''
    fixture = (ROOT / 'tests/boot_ribbon_capture_test.rs').read_text()
    (path / 'src/lib.rs').write_text(modules + fixture)
    priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
    subprocess.run(
        priority + ['cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'),
                    '--lib', '--', '--test-threads=1'],
        check=True,
        env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings', OUT_DIR=tmp),
    )
