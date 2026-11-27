#!/usr/bin/env python3
"""Run the allocator's host checks without interposing its C exports."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
SOURCES = ROOT / "libs/libbreenix-libc/src/stdlib"

with tempfile.TemporaryDirectory(prefix="breenix-malloc-check-") as temporary:
    directory = Path(temporary)
    (directory / "stdlib").mkdir()
    for name in ("malloc", "alloc_cache"):
        source = (SOURCES / f"{name}.rs").read_text().replace("#[no_mangle]", "")
        (directory / "stdlib" / f"{name}.rs").write_text(source)
    harness = directory / "harness.rs"
    fixture = ROOT / "tests/fixtures/libc_malloc_host.rs"
    cases = ROOT / "tests/libc-malloc-interim-cases.rs"
    harness.write_text(f"""
include!({json.dumps(str(fixture))});
use allocation_string as string;

#[allow(dead_code)] // Other C entry points are outside these checks.
mod stdlib {{
    pub(crate) use crate::{{errno, set_errno}};
    pub const EINVAL: i32 = 22;
    pub const ENOMEM: i32 = 12;
    pub mod alloc_cache;
    pub mod malloc;
}}

include!({json.dumps(str(cases))});
""")
    binary = directory / "check"
    subprocess.run(["rustc", "--edition=2021", "--test", "-Dwarnings", str(harness),
                    "-o", str(binary)], check=True)
    subprocess.run([str(binary), "--test-threads=1", *sys.argv[1:]], check=True, timeout=30)
