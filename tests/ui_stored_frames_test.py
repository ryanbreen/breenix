#!/usr/bin/env python3
"""Run the stored-frame production modules on the host, without a kernel build."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def main():
    with tempfile.TemporaryDirectory(prefix="ui-stored-frames-") as directory:
        manifest = Path(directory) / "Cargo.toml"
        manifest.write_text('''[package]
name = "ui-stored-frames-check"
version = "0.1.0"
edition = "2021"
[workspace]
[dependencies]
''' + "\n".join(f'{name} = {{ path = "{ROOT / "libs" / name}" }}'
               for name in ("libpixcheck", "libimg", "libimg-encode")) + f'''
serde_json = "=1.0.154"
[[test]]
name = "ui_stored_frames"
path = "{ROOT / 'tests/ui_stored_frames.rs'}"
''')
        env = dict(os.environ, CARGO_BUILD_JOBS="4", RUSTFLAGS="-Dwarnings")
        command = ["cargo", "test", "--release", "--manifest-path", str(manifest), "--test", "ui_stored_frames"]
        if sys.platform == "darwin":
            command = ["taskpolicy", "-b", *command]
        return subprocess.run(command, cwd=directory, env=env).returncode

if __name__ == "__main__":
    sys.exit(main())
