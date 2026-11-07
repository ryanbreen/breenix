"""U14: fixed-input picture uses the production renderer; live oracle stays exact."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
subject = root / 'userspace/programs/src/suite_breengel/console_picture.rs'
with tempfile.TemporaryDirectory(prefix='console-fixed-picture-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'Cargo.toml').write_text('''[package]
name="console-fixed-picture-check"
version="0.1.0"
edition="2021"
[dependencies]
noto-sans-mono-bitmap={version="0.3",default-features=false,features=["size_16","size_20","size_24","size_32","regular","bold","unicode-basic-latin","unicode-latin-1-supplement","unicode-latin-extended-a","unicode-specials"]}
''')
    modules = f'extern crate alloc;\n#[path="{subject}"] pub mod subject;\n'
    for name in ['boxdraw', 'grid', 'status', 'raster', 'ribbon', 'panel']:
        modules += f'#[path="{root}/kernel/src/console/{name}.rs"] pub mod {name};\n'
    modules += f'#[path="{root}/userspace/programs/src/suite_breengel/window_cases/kernel_restore.rs"] pub mod oracle;\n'
    kit=(root/'userspace/programs/src/suite_breengel/window_cases/kit.rs').read_text()
    pattern=re.search(r'pub\(crate\) fn pattern\([^\n]+\{\n.*?\n\}',kit,re.S)[0]
    pattern=pattern.replace('pub(crate) fn pattern','fn production_pattern')
    scanout=(root/'userspace/programs/src/suite_breengel/window_cases/scanout.rs').read_text()
    atomic=scanout.split('fn scanout_atomic_record()',1)[1].split('fn scanout_window_commits()',1)[0]
    check=atomic[atomic.index('let differ ='):atomic.index('if let Some((x, y)) = differ')]
    check=check.replace('pattern(commit as u32, x, y)','production_pattern(commit as u32, x, y)')
    live_check='fn production_live_mismatch(px:&[u8],width:usize,commit:u64)->Option<(u32,u32)> {'+check+'differ}\n'
    (path / 'src/lib.rs').write_text(modules + pattern + '\n' + live_check +
        (root / 'tests/console_fixed_picture_test.rs').read_text() + '\n' +
        (root / 'tests/atomic_fixed_picture_test.rs').read_text())
    subprocess.run(['taskpolicy', '-b', 'cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'))

panel_case = (root / 'userspace/programs/src/suite_breengel/window_cases/panel.rs').read_text()
assert 'console_picture::picture("panel/kernel-screens", &redrawn, width, height, &serial)' in panel_case
assert 'kernel_restore::compare(&console,&redrawn,width,height,&heading,[(shown_start,shown_end),(again_start,again_end)])' in panel_case
