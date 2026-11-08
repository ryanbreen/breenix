#!/usr/bin/env python3
"""Exercise capture production code with host device boundaries."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="ui-captures-") as directory:
    manifest = Path(directory) / "Cargo.toml"
    manifest.write_text('[package]\nname="ui-capture-check"\nversion="0.1.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=1.0.154"\n' + '\n'.join(f'{name}={{path="{ROOT / "libs" / name}"}}' for name in ("libpixcheck", "breengel-abi", "gpu-abi")) + '\n' + '\n'.join(f'[[test]]\nname="{name}"\npath="{ROOT / "tests" / (name + ".rs")}"' for name in ("ui_capture_marker", "ui_captures")))
    command = ["cargo", "-Z", "bindeps", "test", "--release", "--manifest-path", str(manifest)]
    if sys.platform == "darwin": command = ["taskpolicy", "-b", *command]
    sys.exit(subprocess.run(command, cwd=directory, env=dict(os.environ, CARGO_BUILD_JOBS="4", RUSTFLAGS="-Dwarnings")).returncode)
