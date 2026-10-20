#!/usr/bin/env python3
"""Refuse an ARM64 disk that lacks a binary the testing kernel requires, before the VM starts.

    scripts/check-test-binaries.py DISK

The testing kernel loads every name in kernel/src/boot/test_list.rs (TEST_BINARIES) from the
first of the loader's search directories (search_dirs in kernel/src/main_aarch64.rs) that holds
it, and counts a name it cannot load as a failed test program after the boot. This check reads
the same two lists from the checkout and the directories of DISK with e2fsprogs debugfs, so a
stale disk is refused here rather than one boot later. As the kernel's path lookup does, it
follows symbolic links, at most 8 deep; each name must end at a regular, non-empty file.

Exit 0 accepts the disk; exit 1 names every missing binary; exit 2 is a usage or tool error.
"""
import os
from pathlib import Path
import posixpath
import re
import shutil
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TEST_LIST = ROOT / "kernel/src/boot/test_list.rs"
LOADER = ROOT / "kernel/src/main_aarch64.rs"
MAX_SYMLINK_DEPTH = 8


def quoted_list(path, pattern):
    match = re.search(pattern, path.read_text(encoding="utf-8"), re.DOTALL)
    if not match:
        raise ValueError(f"{path.relative_to(ROOT)}: no list matching {pattern!r}")
    names = re.findall(r'"([^"]+)"', re.sub(r"//[^\n]*", "", match.group(1)))
    if not names:
        raise ValueError(f"{path.relative_to(ROOT)}: the list is empty")
    return names


def required_binaries():
    return quoted_list(TEST_LIST, r"pub const TEST_BINARIES: &\[&str\] = &\[(.*?)\];")


def search_dirs():
    return quoted_list(LOADER, r"let search_dirs = \[(.*?)\];")


def debugfs_path():
    candidates = [os.environ.get("BREENIX_DEBUGFS"),
                  "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
                  "/usr/local/opt/e2fsprogs/sbin/debugfs",
                  "/sbin/debugfs", "/usr/sbin/debugfs", shutil.which("debugfs")]
    tool = next((p for p in candidates if p and os.access(p, os.X_OK)), None)
    if tool is None:
        raise FileNotFoundError("needs e2fsprogs debugfs")
    return tool


class Image:
    def __init__(self, disk, debugfs):
        self.disk, self.debugfs, self.listings = str(disk), debugfs, {}

    def run(self, command):
        result = subprocess.run([self.debugfs, "-R", command, self.disk], capture_output=True)
        if result.returncode != 0:
            raise RuntimeError(f"debugfs could not read {self.disk}: {result.stderr.decode(errors='replace').strip()}")
        return result.stdout

    def listing(self, directory):
        """Map names in a directory inode to (octal mode, size, inode)."""
        if directory not in self.listings:
            entries = {}
            # inode  mode  (type)  uid  gid  size  date time  name
            for line in self.run(f"ls -l <{directory}>").decode(errors="replace").splitlines():
                fields = line.split(None, 8)
                if len(fields) == 9 and fields[1].isdigit() and fields[5].isdigit():
                    entries[fields[8]] = (fields[1], int(fields[5]), int(fields[0]))
            self.listings[directory] = entries
        return self.listings[directory]

    def link_target(self, path):
        stat = self.run(f"stat {path}").decode(errors="replace")
        fast = re.search(r'Fast link dest: "(.*)"', stat)
        return fast.group(1) if fast else self.run(f"cat {path}").decode(errors="replace")

    def resolve(self, path, depth=0):
        """Resolve each component, including intermediate links, as ext2 resolve_path does."""
        if depth > MAX_SYMLINK_DEPTH:
            return None
        entry = self.listing(2)["."]
        components = [part for part in path.split("/") if part]
        for index, component in enumerate(components):
            mode, size, inode = entry
            if not stat.S_ISDIR(int(mode, 8)):
                return None
            if component == ".":
                continue
            entry = self.listing(inode).get(component)
            if entry is None:
                return None
            if stat.S_ISLNK(int(entry[0], 8)):
                target = self.link_target(f"<{entry[2]}>")
                if not target.startswith("/"):
                    target = "/" + "/".join(components[:index]) + "/" + target
                remaining = components[index + 1:]
                if remaining:
                    target += "/" + "/".join(remaining)
                return self.resolve(target, depth + 1)
        return entry

    def loads(self, name, directories):
        """Validate the first resolved search path; an unusable entry shadows later copies."""
        for directory in directories:
            entry = self.resolve(posixpath.join(directory, name))
            if entry is not None:
                mode, size, inode = entry
                return stat.S_ISREG(int(mode, 8)) and size > 0
        return False


def missing_binaries(disk, names, directories, debugfs):
    image = Image(disk, debugfs)
    return [name for name in names if not image.loads(name, directories)]


def main(argv):
    if len(argv) != 2:
        print("usage: scripts/check-test-binaries.py DISK", file=sys.stderr)
        return 2
    disk = Path(argv[1])
    if not disk.is_file():
        print(f"check-test-binaries: no disk at {disk}", file=sys.stderr)
        return 2
    try:
        names, directories = required_binaries(), search_dirs()
        missing = missing_binaries(disk, names, directories, debugfs_path())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"check-test-binaries: {error}", file=sys.stderr)
        return 2
    if missing:
        print(f"check-test-binaries: {disk} lacks {len(missing)} of the {len(names)} testing binaries "
              f"in {', '.join(directories)}: {', '.join(missing)}", file=sys.stderr)
        print("check-test-binaries: rebuild the disk (run without --no-build)", file=sys.stderr)
        return 1
    print(f"==> Test disk has all {len(names)} testing binaries")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
