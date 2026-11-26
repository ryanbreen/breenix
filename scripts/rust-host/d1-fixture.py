#!/usr/bin/env python3
"""Populate the sized E24 scratch companion with the verified D1 inputs."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REPLAY = ROOT / "toolchain/rust-host/d1/x86_64-mac.json"
MIB = 1 << 20


def git(*args):
    return subprocess.check_output(["git", "-C", ROOT, *args])


def facts(path):
    with path.open("rb") as stream:
        return {"size": path.stat().st_size, "sha256": hashlib.file_digest(stream, "sha256").hexdigest()}


def main(tree, mkfs, disk):
    replay = json.loads(REPLAY.read_text())
    for name, expected in replay["workspace"].items():
        if git("rev-parse", f"HEAD:toolchain/{name}").decode().strip() != expected:
            raise ValueError(f"toolchain/{name} changed since the D1 Mac twin replay")
    for name, expected in replay["inputs"].items():
        if facts(tree / name) != expected:
            raise ValueError(f"{name} differs from the D1 Mac twin input")

    with subprocess.Popen(["git", "-C", ROOT, "archive", "HEAD", "toolchain"], stdout=subprocess.PIPE) as tar:
        subprocess.run(["tar", "-xf", "-", "-C", tree], stdin=tar.stdout, check=True)
        tar.stdout.close()
        if tar.wait():
            raise ValueError("cannot archive the D1 workspace")
    (tree / "breenix-d1").write_bytes(b"e25-d1\n")
    (tree / "fixture/cargo-home/registry").mkdir(parents=True)
    files = {str(path.relative_to(tree)): facts(path) for path in sorted(tree.rglob("*")) if path.is_file()}
    payload = sum(item["size"] for item in files.values())
    # The Mac receipt measures logical retained data. Reserve four times the
    # total observed growth plus 128 MiB for transient files and filesystem data.
    scratch = sum(item["scratch_growth_bytes"] for item in replay["packages"])
    size = ((payload + 4 * scratch + 128 * MIB + MIB - 1) // MIB) * MIB
    subprocess.run([str(mkfs), "--size", str(size), "--populate", str(tree), str(disk)], check=True)
    record = {"commit": git("rev-parse", "HEAD").decode().strip(), "disk": facts(disk),
              "payload_bytes": payload, "mac_scratch_growth_bytes": scratch,
              "mac_replay": facts(REPLAY), "files": files}
    disk.with_name("d1-build.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main(*(Path(arg) for arg in sys.argv[1:]))
