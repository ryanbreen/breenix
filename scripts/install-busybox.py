#!/usr/bin/env python3
"""Install the repo's pinned BusyBox, rejecting corrupt or wrong-arch inputs."""
import hashlib
import json
from pathlib import Path
import shutil
import sys

root = Path(__file__).resolve().parent.parent
arch = sys.argv[1]
vendor = root / "vendor/busybox"
entry = json.loads((vendor / "manifest.json").read_text())[arch]
source = vendor / entry["file"]
data = source.read_bytes()
if hashlib.sha256(data).hexdigest() != entry["sha256"]:
    sys.exit(f"BusyBox checksum mismatch: {source.name}")
if data[:4] != b"\x7fELF" or int.from_bytes(data[18:20], "little") != entry["machine"]:
    sys.exit(f"BusyBox ELF architecture mismatch: {source.name}")
destination = root / "userspace/programs"
if arch == "aarch64":
    destination /= "aarch64"
destination.mkdir(parents=True, exist_ok=True)
shutil.copyfile(source, destination / "busybox.elf")
print(f"Installed pinned BusyBox 1.37.0 ({arch}, sha256={entry['sha256']})")
