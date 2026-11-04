"""B36a: judge restored pixels with a live, independently checked heading."""
from pathlib import Path
import os
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
subject = root / 'userspace/programs/src/suite_breengel/window_cases/kernel_restore.rs'
assert subject.is_file(), 'B36a kernel restoration oracle is missing'
with tempfile.TemporaryDirectory(prefix='console-restore-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'Cargo.toml').write_text('''[package]
name="console-restore-check"
version="0.1.0"
edition="2021"
[dependencies]
noto-sans-mono-bitmap={version="0.3",default-features=false,features=["size_16","size_20","size_24","size_32","regular","bold","unicode-basic-latin","unicode-latin-1-supplement","unicode-latin-extended-a","unicode-specials"]}
''')
    modules = f'extern crate alloc;\n#[path="{subject}"] mod subject;\n'
    for name in ['boxdraw', 'grid', 'status', 'raster', 'ribbon', 'panel']:
        modules += f'#[path="{root}/kernel/src/console/{name}.rs"] pub mod {name};\n'
    (path / 'src/lib.rs').write_text(modules + (root / 'tests/console_restore_test.rs').read_text())
    subprocess.run(['taskpolicy', '-b', 'cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'))
