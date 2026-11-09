"""Exercise scanner boundaries using the production capture and suite parser."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root / 'kernel/src/graphics/boot_ribbon.rs').read_text()
# On the old implementation the missing state is observable as absent, so the
# same executable check reports the old behavior instead of failing to compile.
probe = '''
#[cfg(test)] mod boundary_checks {
    use super::*;
    fn emit(line: &str) { for b in line.bytes() { byte(b); } drain(); }
    #[test] fn boundaries_and_lap() {
        emit("boot\n");
        let first = TAIL.load(Ordering::Acquire);
        emit("SUITE first START cases=1\n");
        emit("BOOT_LATE_READY\n");
        assert!(!reached()[0], "a stage after the first START must not latch");
        emit("SUITE invalid START cases=no\n");
        let latest = TAIL.load(Ordering::Acquire);
        emit("SUITE second START cases=1\n");
        assert_eq!(offsets(), (first, latest));
        for _ in 0..(11 * 1024 * 1024 / 10) { byte(b'x'); }
        drain();
        assert!(suite_status().is_none(), "an overwritten scanner must refuse status");
        drain();
        assert!(suite_status().is_none(), "lap must stay sticky");
    }
    OFFSETS
}
'''
if 'static FIRST_START:' in source and 'static LATEST_START:' in source:
    offsets = 'fn offsets() -> (usize, usize) { (FIRST_START.load(Ordering::Acquire), LATEST_START.load(Ordering::Acquire)) }'
else:
    offsets = 'fn offsets() -> (usize, usize) { (usize::MAX, usize::MAX) }'
with tempfile.TemporaryDirectory(prefix='boot-ribbon-boundaries-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'Cargo.toml').write_text('[package]\nname="ribbon-boundaries"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nspin="0.9.8"\n')
    (path / 'boot_ribbon.rs').write_text('&["BOOT_LATE_READY"]\n')
    (path / 'ribbon.rs').write_text(source + probe.replace('OFFSETS', offsets))
    (path / 'src/lib.rs').write_text(f'''extern crate alloc;
pub fn arch_interrupts_enabled() -> bool {{ true }}
pub mod time {{ pub fn get_monotonic_time_ns() -> (u64,u64) {{ (0,0) }} }}
pub mod console {{ #[path="{root}/kernel/src/console/status.rs"] pub mod status; }}
pub mod graphics {{
#[path="{root}/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;
#[path="{path}/ribbon.rs"] pub mod boot_ribbon;
}}
''')
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                   ['cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings', OUT_DIR=tmp))
