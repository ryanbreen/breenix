#!/usr/bin/env python3
"""Install the repo's pinned BusyBox, rejecting corrupt or wrong-arch inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("arch", choices=("x86_64", "aarch64"))
parser.add_argument("--verify", action="store_true", help="verify without installing")
args = parser.parse_args()
arch = args.arch
vendor = root / "vendor/busybox"
entry = json.loads((vendor / "manifest.json").read_text())[arch]
source = vendor / entry["file"]
data = source.read_bytes()
if hashlib.sha256(data).hexdigest() != entry["sha256"]:
    sys.exit(f"BusyBox checksum mismatch: {source.name}")
if data[:4] != b"\x7fELF" or int.from_bytes(data[18:20], "little") != entry["machine"]:
    sys.exit(f"BusyBox ELF architecture mismatch: {source.name}")
if args.verify:
    print(f"Verified pinned BusyBox 1.37.0 ({arch})")
    sys.exit(0)
destination = root / "userspace/programs"
if arch == "aarch64":
    destination /= "aarch64"
destination.mkdir(parents=True, exist_ok=True)
shutil.copyfile(source, destination / "busybox.elf")
print(f"Installed pinned BusyBox 1.37.0 ({arch}, sha256={entry['sha256']})")
