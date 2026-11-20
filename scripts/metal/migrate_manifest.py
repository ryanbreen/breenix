#!/usr/bin/env python3
"""Emit the ext2 side of docs/fs/migration-manifest.md using read-only debugfs."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from metal_root import require_e2fsprogs


# A name may contain whitespace or arbitrary non-NUL bytes. In ls -p
# the slash ends the name field, so splitting on line breaks loses names.
ENTRY = re.compile(rb"/([0-9]+)/[0-7]+/[0-9]+/[0-9]+/([^/\x00]*)/[0-9]*/\n")
STAT = re.compile(rb"\AInode:[ \t]+([0-9]+)[ \t]+Type: (.+?)[ \t]+Mode:[ \t]+(0?[0-7]{4})\b")
OWNER_SIZE = re.compile(rb"^User:\s+([0-9]+)\s+Group:\s+([0-9]+)[^\n]*?\bSize:\s+([0-9]+)\b", re.M)
MTIME = re.compile(rb"^[ \t]*mtime: 0x([0-9a-fA-F]{8})(?::([0-9a-fA-F]{8}))? -- ", re.M)
DEVICE = re.compile(rb"^Device major/minor number: ([0-9]+):([0-9]+)\b", re.M)
KINDS = {
    b"regular": "reg",
    b"directory": "dir",
    b"symlink": "symlink",
    b"character special": "chr",
    b"block special": "blk",
    b"FIFO": "fifo",
    b"socket": "sock",
}


def check_debugfs(command, status, stderr):
    first_line, _, remaining = stderr.partition(b"\n")
    if re.fullmatch(rb"debugfs [0-9].*", first_line):
        stderr = remaining
    if status or stderr.strip():
        detail = stderr.decode(errors="replace").strip()
        raise ValueError(f"debugfs {command}: {detail or f'exit {status}'}")


class Ext2:
    def __init__(self, debugfs, image):
        self.debugfs = debugfs
        self.image = image
        self.env = dict(os.environ, LC_ALL="C", TZ="UTC", DEBUGFS_PAGER="cat")

    def argv(self, command):
        return [self.debugfs, "-R", command, self.image]

    def read(self, command):
        result = subprocess.run(self.argv(command), capture_output=True, env=self.env)
        check_debugfs(command, result.returncode, result.stderr)
        return result.stdout

    def children(self, inode):
        listing = self.read(f"ls -p <{inode}>")
        offset = 0
        entries = {}
        while offset < len(listing) and listing[offset] != 10:
            entry = ENTRY.match(listing, offset)
            if not entry:
                raise ValueError(f"invalid directory listing for inode {inode}")
            offset = entry.end()
            child = int(entry[1])
            # Preallocated directory blocks can contain unused entries.
            if child:
                name = entry[2]
                if not name or name in entries:
                    raise ValueError(f"invalid directory listing for inode {inode}")
                entries[name] = child
        if listing[offset:].strip(b"\n") or entries.pop(b".", None) != inode or b".." not in entries:
            raise ValueError(f"invalid directory listing for inode {inode}")
        del entries[b".."]
        return entries

    def digest(self, inode, size):
        command = f"cat <{inode}>"
        count = 0
        digest = hashlib.sha256()
        # Keep large file data out of memory and let the child write diagnostics
        # independently of the stdout pipe.
        with tempfile.TemporaryFile() as errors:
            with subprocess.Popen(self.argv(command), stdout=subprocess.PIPE,
                                  stderr=errors, env=self.env) as process:
                for chunk in iter(lambda: process.stdout.read(256 * 1024), b""):
                    digest.update(chunk)
                    count += len(chunk)
                status = process.wait()
            errors.seek(0)
            check_debugfs(command, status, errors.read())
        if count != size:
            raise ValueError(f"short or overlong content for inode {inode}: {count} != {size}")
        return digest.hexdigest()

    def metadata(self, inode):
        output = self.read(f"stat <{inode}>")
        stat, _, fast_link = output.partition(b'\nFast link dest: "')
        header = STAT.match(stat)
        owner_size = OWNER_SIZE.search(stat)
        mtime = MTIME.search(stat)
        if not header or int(header[1]) != inode or not owner_size or not mtime:
            raise ValueError(f"invalid stat output for inode {inode}")
        kind = KINDS.get(header[2])
        if kind is None:
            raise ValueError(f"unsupported file type for inode {inode}: {header[2]!r}")

        seconds = int(mtime[1], 16)
        nanos = 0
        if mtime[2] is not None:
            extra = int(mtime[2], 16)
            # The extended timestamp printed by debugfs combines a signed
            # low word, an epoch extension, and nanoseconds.
            seconds = (seconds ^ (1 << 31)) - (1 << 31)
            seconds += (extra & 3) << 32
            nanos = extra >> 2
            if nanos >= 1_000_000_000:
                raise ValueError(f"invalid mtime for inode {inode}")

        uid, gid, size = map(int, owner_size.groups())
        digest = "-"
        if kind == "symlink" and fast_link:
            target = fast_link[:size]
            if fast_link[size:] != b'"\n' or b"\x00" in target:
                raise ValueError(f"invalid fast symlink target for inode {inode}")
            digest = hashlib.sha256(target).hexdigest()
        elif kind in ("reg", "symlink"):
            digest = self.digest(inode, size)
        else:
            size = 0
            if kind in ("chr", "blk"):
                number = DEVICE.search(stat)
                if number is None:
                    raise ValueError(f"missing device number for inode {inode}")
                kind = f"{kind}:{int(number[1])}:{int(number[2])}"

        mode = int(header[3], 8)
        return kind, f"{mode:04o}", uid, gid, size, f"{seconds}.{nanos:09d}", digest

    def rows(self, subtree):
        if not subtree.startswith(b"/"):
            raise ValueError("--subtree must be an absolute directory path")
        root = 2
        for component in filter(None, subtree.split(b"/")):
            children = self.children(root)
            if component not in children:
                raise ValueError(f"no subtree directory {subtree!r}")
            root = children[component]

        data = {}
        paths = []
        directories = set()
        pending = [(b"/", root)]
        while pending:
            path, inode = pending.pop()
            if inode not in data:
                data[inode] = self.metadata(inode)
            kind = data[inode][0]
            if kind == "dir":
                if inode in directories:
                    raise ValueError(f"directory cycle or alias at inode {inode}")
                directories.add(inode)
                prefix = b"/" if path == b"/" else path + b"/"
                pending.extend((prefix + name, child) for name, child in self.children(inode).items())
            elif path == b"/":
                raise ValueError(f"--subtree is not a directory: {subtree!r}")
            paths.append((path, inode))
        paths.sort()

        first_path = {}
        groups = {}
        for path, inode in paths:
            if data[inode][0] != "dir":
                first = first_path.setdefault(inode, path)
                if first != path:
                    groups[inode] = first.hex()

        for path, inode in paths:
            kind, mode, uid, gid, size, mtime, digest = data[inode]
            group = groups.get(inode, "-")
            yield f"{path.hex()}\t{kind}\t{mode}\t{uid}\t{gid}\t{group}\t{size}\t{mtime}\t{digest}\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("filesystem", choices=["ext2"])
    parser.add_argument("image", type=Path)
    parser.add_argument("--subtree", default="/", metavar="PATH")
    args = parser.parse_args()
    try:
        if not args.image.is_file():
            raise ValueError("IMAGE must be a regular image file")
        debugfs, = require_e2fsprogs("debugfs")
        reader = Ext2(debugfs, args.image.resolve())
        for row in reader.rows(os.fsencode(args.subtree)):
            sys.stdout.buffer.write(row.encode("ascii"))
        sys.stdout.buffer.flush()
    except (OSError, ValueError) as error:
        parser.exit(1, f"migrate_manifest.py: {error}\n")


if __name__ == "__main__":
    main()
