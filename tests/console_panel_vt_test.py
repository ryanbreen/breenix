"""Exercise panel publication across foreground VTs with the production Screen methods."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

source = (ROOT / 'kernel/src/console/mod.rs').read_text()
refresh = item(source, '    fn refresh_panel(')
update = item(source, '    fn update(')
# Earlier code has no foreground argument; keep the same behavioural invocation.
call = 'self.refresh_panel();' if 'fn refresh_panel(&mut self)' in refresh else 'self.refresh_panel(snapshot.fg);'
fixture = (ROOT / 'tests/console_panel_vt_test.rs').read_text()
fixture = fixture.replace('REFRESH_METHOD', refresh).replace('UPDATE_METHOD', update).replace('REFRESH_CALL', call)
if 'fn may_draw_panel(' in source:
    fixture += item(source, 'fn may_draw_panel(')
with tempfile.TemporaryDirectory(prefix='console-panel-vt-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    manifest = '\n'.join([
        '[package]', 'name="console-panel-vt-check"', 'version="0.1.0"', 'edition="2021"',
        '[dependencies]',
        'noto-sans-mono-bitmap={version="0.3",default-features=false,features=["size_16","size_20","size_24","size_32","regular","bold","unicode-basic-latin","unicode-latin-1-supplement","unicode-latin-extended-a","unicode-specials"]}',
    ])
    (path / 'Cargo.toml').write_text(manifest)
    modules = 'extern crate alloc;\n'
    for name in ['boxdraw', 'grid', 'status', 'raster', 'ribbon', 'panel']:
        modules += f'#[path="{ROOT}/kernel/src/console/{name}.rs"] pub mod {name};\n'
    (path / 'src/lib.rs').write_text(modules + fixture)
    priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
    subprocess.run(priority + ['cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'))
