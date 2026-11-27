#!/usr/bin/env python3
"""Pack the BreenFS extents for the Mac-matched self-host workspace."""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[2]
MAC_REFERENCE = ROOT / "toolchain/rust-host/selfhost/x86_64-mac.json"
MIB = 1 << 20
BLOCK = 4096


def git(*args):
    return subprocess.check_output(["git", "-C", ROOT, *args], text=True).strip()


def facts(path):
    with path.open("rb") as stream:
        return {"size": path.stat().st_size, "sha256": hashlib.file_digest(stream, "sha256").hexdigest()}


def main(tree, mkfs, disk, arch="x86_64"):
    reference = MAC_REFERENCE if arch == "x86_64" else ROOT / f"toolchain/rust-host/selfhost/{arch}-mac.json"
    receipt = json.loads(reference.read_text())
    commit = git("rev-parse", "HEAD")
    workspace = []
    for name, expected in receipt["workspace"].items():
        path = f"toolchain/{name}"
        if git("rev-parse", f"{commit}:{path}") != expected:
            raise ValueError(f"{path} changed since the self-host Mac twin replay")
        workspace.append(path)

    tree.mkdir()
    with subprocess.Popen(["git", "-C", ROOT, "archive", commit, *workspace],
                          stdout=subprocess.PIPE) as tar:
        subprocess.run(["tar", "-xf", "-", "-C", tree], stdin=tar.stdout, check=True)
        tar.stdout.close()
        if tar.wait():
            raise ValueError("cannot archive the self-host workspace")
    (tree / "breenix-self-host").write_bytes(b"e25-self-host\n")
    files = {str(path.relative_to(tree)): facts(path) for path in sorted(tree.rglob("*")) if path.is_file()}
    payload = sum(item["size"] for item in files.values())
    # Ruling 135(2a) rounds twice D1's 22,968,684 measured bytes up to 64 MiB.
    size = ((payload + 64 * MIB + BLOCK - 1) // BLOCK) * BLOCK
    image = tree.with_suffix(".img")
    extents = subprocess.check_output([str(mkfs), "--extents", "--size", str(size),
                                       "--populate", str(tree), str(image)], text=True)
    end = 0
    with image.open("rb") as source, disk.open("wb") as packed:
        for line in extents.splitlines():
            first, count = (int(token) * BLOCK for token in line.split())
            if first < end or count <= 0 or first + count > size:
                raise ValueError("invalid extent from self-host scratch formatter")
            packed.write(struct.pack("<QQ", first, count))
            source.seek(first)
            remaining = count
            while remaining:
                data = source.read(min(remaining, MIB))
                if not data:
                    raise ValueError("short self-host scratch image")
                packed.write(data)
                remaining -= len(data)
            end = first + count
    if end != size or image.stat().st_size != size:
        raise ValueError("self-host scratch formatter omitted the closing superblock ring")
    if arch == "x86_64":
        image.unlink()
    record = {"commit": commit, "disk": facts(disk), "scratchBytes": size, "payload_bytes": payload,
              "mac_replay": facts(reference), "files": files}
    disk.with_name("self-host-build.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("x86_64", "aarch64"), default="x86_64")
    parser.add_argument("tree", type=Path)
    parser.add_argument("mkfs", type=Path)
    parser.add_argument("disk", type=Path)
    args = parser.parse_args()
    main(args.tree, args.mkfs, args.disk, args.arch)
