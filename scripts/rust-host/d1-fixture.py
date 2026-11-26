#!/usr/bin/env python3
"""Pack the formatted BreenFS extents of the D1 workspace and scratch volume."""

import hashlib
import json
import struct
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
REPLAY = ROOT / "toolchain/rust-host/d1/x86_64-mac.json"
MIB = 1 << 20
BLOCK = 4096


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
    workspace = [f"toolchain/{name}" for name in ("Cargo.toml", "Cargo.lock")]
    for name in replay["workspace"].keys() - {"Cargo.toml", "Cargo.lock"}:
        workspace.extend([f"toolchain/{name}/Cargo.toml", f"toolchain/{name}/src"])
    tree.mkdir()
    with subprocess.Popen(["git", "-C", ROOT, "archive", "HEAD", *workspace], stdout=subprocess.PIPE) as tar:
        subprocess.run(["tar", "-xf", "-", "-C", tree], stdin=tar.stdout, check=True)
        tar.stdout.close()
        if tar.wait():
            raise ValueError("cannot archive the D1 workspace")
    (tree / "breenix-d1").write_bytes(b"e25-d1\n")
    files = {str(path.relative_to(tree)): facts(path) for path in sorted(tree.rglob("*")) if path.is_file()}
    payload = sum(item["size"] for item in files.values())
    # The Mac receipt measures logical retained data. Reserve four times the
    # total observed growth plus 128 MiB for transient files and filesystem data.
    scratch = sum(item["scratch_growth_bytes"] for item in replay["packages"])
    size = ((payload + 4 * scratch + 128 * MIB + MIB - 1) // MIB) * MIB
    image = tree.with_suffix(".img")
    extents = subprocess.check_output([str(mkfs), "--extents", "--size", str(size),
                                       "--populate", str(tree), str(image)], text=True)
    # The formatter lists its written blocks, including cleared superblock
    # slots. Free blocks from the previous boot can remain unreferenced.
    end = 0
    with image.open("rb") as source, disk.open("wb") as packed:
        for line in extents.splitlines():
            first, count = (int(token) * BLOCK for token in line.split())
            if first < end or count <= 0 or first + count > size:
                raise ValueError("invalid extent from D1 scratch formatter")
            packed.write(struct.pack("<QQ", first, count))
            source.seek(first)
            remaining = count
            while remaining:
                data = source.read(min(remaining, MIB))
                if not data:
                    raise ValueError("short D1 scratch image")
                packed.write(data)
                remaining -= len(data)
            end = first + count
    if end != size or image.stat().st_size != size:
        raise ValueError("D1 scratch formatter omitted the closing superblock ring")
    image.unlink()
    record = {"commit": git("rev-parse", "HEAD").decode().strip(), "disk": facts(disk),
              "scratchBytes": size, "payload_bytes": payload, "mac_scratch_growth_bytes": scratch,
              "mac_replay": facts(REPLAY), "files": files}
    disk.with_name("d1-build.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main(*(Path(arg) for arg in sys.argv[1:]))
