#!/usr/bin/env python3
"""Exercise the session cursor policy and the shared canonical renderer on the host."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="ui-canonical-") as directory:
    manifest = Path(directory) / "Cargo.toml"
    manifest.write_text('[package]\nname="ui-canonical-check"\nversion="0.1.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=1.0.154"\n' + '\n'.join(f'{name}={{path="{ROOT / "libs" / name}"}}' for name in ("libpixcheck", "libgfx-ref")) + f'\n[[test]]\nname="ui_canonical"\npath="{ROOT / "tests/ui_canonical.rs"}"\n')
    command = ["cargo", "-Z", "bindeps", "test", "--release", "--manifest-path", str(manifest), *sys.argv[1:]]
    if sys.platform == "darwin": command = ["taskpolicy", "-b", *command]
    sys.exit(subprocess.run(command, cwd=directory, env=dict(os.environ, CARGO_BUILD_JOBS="4", RUSTFLAGS="-Dwarnings")).returncode)
