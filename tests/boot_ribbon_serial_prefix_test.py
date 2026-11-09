"""Check the no-START head boundary against the production capture."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='boot-ribbon-prefix-') as tmp:
    path = Path(tmp)
    (path / 'src').mkdir()
    (path / 'Cargo.toml').write_text('[package]\nname="ribbon-prefix"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nspin="0.9.8"\n')
    (path / 'boot_ribbon.rs').write_text('&["BOOT_READY"]\n')
    (path / 'src/lib.rs').write_text(f'''extern crate alloc;
pub fn arch_interrupts_enabled() -> bool {{ true }}
pub mod time {{ pub fn get_monotonic_time_ns() -> (u64,u64) {{ (0,0) }} }}
pub mod console {{ #[path="{root}/kernel/src/console/status.rs"] pub mod status; }}
pub mod graphics {{
#[path="{root}/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;
#[path="{root}/kernel/src/graphics/boot_ribbon.rs"] pub mod boot_ribbon;
}}
#[test] fn head_overflow_without_start() {{
    use graphics::boot_ribbon::*;
    for _ in 0..250 * 1024 {{ byte(b'x'); }}
    assert!(matches!(serial(), Err(-75)), "head overflow must be reported");
}}
''')
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                   ['cargo', 'test', '--manifest-path', str(path / 'Cargo.toml'), '--lib'],
                   check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings', OUT_DIR=tmp))
