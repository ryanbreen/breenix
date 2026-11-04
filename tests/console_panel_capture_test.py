"""Judge the first restored panel after a producer backlog larger than one batch."""
from pathlib import Path
import os
import subprocess
import tempfile

from gt710_gr_live_unwind_test import item

root = Path(__file__).resolve().parents[1]
refresh = item((root / 'kernel/src/console/mod.rs').read_text(), '    fn refresh_panel(')
with tempfile.TemporaryDirectory(prefix='console-panel-capture-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'boot_ribbon.rs').write_text('&["kernel ready"]')
    (path / 'Cargo.toml').write_text('''[package]
name="console-panel-capture-check"
version="0.1.0"
edition="2021"
[dependencies]
noto-sans-mono-bitmap={version="0.3",default-features=false,features=["size_16","size_20","size_24","size_32","regular","bold","unicode-basic-latin","unicode-latin-1-supplement","unicode-latin-extended-a","unicode-specials"]}
''')
    modules = 'extern crate alloc;\nextern crate self as spin;\n'
    for name in ['boxdraw', 'grid', 'status', 'raster', 'ribbon', 'panel']:
        modules += f'#[path="{root}/kernel/src/console/{name}.rs"] pub mod {name};\n'
    modules += f'#[path="{root}/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;\n'
    modules += f'#[path="{root}/kernel/src/graphics/boot_ribbon.rs"] pub mod boot_ribbon;\n'
    fixture = r'''
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_,T>> { self.0.try_lock().ok() }
}
pub fn arch_interrupts_enabled() -> bool { true }
pub mod console { pub use crate::status; }
pub mod time {
    use std::sync::atomic::{AtomicU64,Ordering};
    pub static NOW: AtomicU64 = AtomicU64::new(1000);
    pub fn get_monotonic_time_ns() -> (u64,u64) {
        let ms=NOW.load(Ordering::Relaxed); (ms/1000,(ms%1000)*1_000_000)
    }
}
struct Screen {
    layout: raster::Layout, panel: Option<panel::Panel>, panel_dirty: bool,
    panel_updated_ms: Option<u64>, ribbon_drop: bool,
}
impl Screen { REFRESH }
#[test]
fn first_restored_frame_includes_the_captured_done_and_elapsed_time() {
    let layout=raster::Layout { width:1280,height:800,bytes_per_pixel:4,stride_bytes:5120,
        reserved:255,bgr:true,font:raster::Font::for_display(1280,800) };
    for b in std::iter::repeat(b'x').take(8191).chain(b"\nSUITE console START cases=1\n".iter().copied()) {
        boot_ribbon::byte(b);
    }
    time::NOW.store(62000,std::sync::atomic::Ordering::Relaxed);
    for &b in b"SUITE console CASE text/a PASS ms=1\nSUITE console DONE passed=1 failed=0 skipped=0 total=1\n" {
        boot_ribbon::byte(b);
    }
    time::NOW.store(999999,std::sync::atomic::Ordering::Relaxed);
    let mut screen=Screen {layout,panel:None,panel_dirty:false,panel_updated_ms:None,ribbon_drop:false};
    screen.refresh_panel(0);
    let reached=boot_ribbon::reached();
    let suite=boot_ribbon::suite_status().unwrap();
    assert_eq!(suite.elapsed(999999),(1,1));
    let expected=panel::Panel::new(&screen.layout,suite,reached,false,999999);
    assert!(screen.panel==expected,"the first restored frame used suite state from before the backlog drain");
}
'''.replace('REFRESH', refresh)
    (path / 'src/lib.rs').write_text(modules + fixture)
    subprocess.run(['taskpolicy', '-b', 'cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings', OUT_DIR=tmp))
