"""Check the reference receipt against both Mac twin builds and their tool inputs."""

import hashlib
import json
from pathlib import Path
import sys


def check_file(path, expected):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    actual = {"size": path.stat().st_size, "sha256": digest}
    if actual != expected:
        sys.exit(f"{path}: expected {expected}, got {actual}")


def check_receipt(receipt_path, work, outputs):
    receipt = json.loads(receipt_path.read_text())
    inputs = receipt["inputs"]
    check_file(work / "host-target/release/bld", inputs["host-bld"])
    for build in ("one", "two"):
        root = work / build
        check_file(root / "fixture/bin/rustc", inputs["mac-twin-rustc"])
        check_file(root / "fixture/bin/cargo", inputs["mac-cargo"])
        profile = root / "run/target/x86_64-breenix/selfhost"
        for name in outputs:
            check_file(profile / name, receipt["outputs"][name])


if __name__ == "__main__":
    receipt_path, work = map(Path, sys.argv[1:3])
    check_receipt(receipt_path, work, sys.argv[3:])
