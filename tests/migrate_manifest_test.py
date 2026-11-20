"""Compare the host migration readers on disposable images."""
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
METAL = REPO / "scripts" / "metal"
TOOLS = Path(os.environ.get("BREENFS_TOOLS_DIR", REPO / "tools/breenfs/target/debug"))
sys.path.insert(0, str(METAL))
from metal_root import require_e2fsprogs

TIME = 1234567890


class MigrationManifestTest(unittest.TestCase):
    def run_tool(self, *args):
        proc = subprocess.run(args, capture_output=True, timeout=20)
        self.assertEqual(proc.returncode, 0, proc.stderr.decode(errors="replace"))
        return proc.stdout

    def test_pre_epoch_mtime_in_a_128_byte_inode(self):
        mke2fs, debugfs = require_e2fsprogs("mke2fs", "debugfs")
        with tempfile.TemporaryDirectory() as tmp:
            image = Path(tmp) / "ext2.img"
            self.run_tool(mke2fs, "-q", "-t", "ext2", "-I", "128", "-F", image, "4096")
            self.run_tool(debugfs, "-w", "-R", "set_inode_field <2> mtime 0xfffffff7", image)
            stat = self.run_tool(debugfs, "-R", "stat <2>", image)
            self.assertRegex(stat, rb"\bmtime: 0xfffffff7 -- ")
            manifest = self.run_tool(sys.executable, METAL / "migrate_manifest.py", "ext2", image)
            self.assertEqual(manifest.splitlines()[0].split(b"\t")[7], b"-9.000000000")

    def test_ext2_and_breenfs_compare_logical_trees(self):
        mke2fs, debugfs = require_e2fsprogs("mke2fs", "debugfs")
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            source = base / "tree"
            source.mkdir()
            nested = source / "nested"
            nested.mkdir()
            child = nested / "tab\tline\nbreak"
            child.write_bytes(b"hello")
            program = source / "program"
            program.write_bytes(b"binary")
            program.chmod(0o4755)
            alias = source / "alias"
            os.link(program, alias)
            zero = source / "zero"
            zero.touch()
            os.truncate(zero, 8192)
            symlink = source / "sym"
            target = 'nested/tab\tline\nbreak'
            symlink.symlink_to(target)
            paths = [source, nested, child, program, alias, zero, symlink]
            for path in paths:
                os.utime(path, (TIME, TIME), follow_symlinks=False)

            ext2 = base / "ext2.img"
            self.run_tool(mke2fs, "-q", "-t", "ext2", "-b", "4096", "-d", source,
                          "-E", "root_owner=0:0", "-F", ext2, "16384")
            for command in ("rmdir /lost+found", f"set_inode_field <2> mtime @{TIME}",
                            "set_inode_field /program uid 1000", "fallocate /zero 0 1"):
                self.run_tool(debugfs, "-w", "-R", command, ext2)
            self.assertRegex(self.run_tool(debugfs, "-R", "stat /zero", ext2),
                             rb"Blockcount:\s+16\b")

            owners = base / "owners"
            with owners.open("wb") as out:
                for path in (nested, program, zero, symlink):
                    info = path.lstat()
                    uid = 1000 if path == program else info.st_uid
                    mode = stat.S_IMODE(info.st_mode)
                    out.write(f"{uid}:{info.st_gid}:{mode:o}\t".encode()
                              + os.fsencode("/" + str(path.relative_to(source))) + b"\n")
            # The owners file is line-delimited. Keep the formatter's default
            # IDs for the newline name, and set those in ext2 by inode.
            listing = self.run_tool(debugfs, "-R", "ls -p /nested", ext2)
            child_entry = re.search(rb"(?:^|\n)/([0-9]+)/[0-7]+/\d+/\d+/tab\tline\nbreak/", listing)
            self.assertIsNotNone(child_entry, listing)
            for field in ("uid", "gid"):
                self.run_tool(debugfs, "-w", "-R",
                              f"set_inode_field <{child_entry[1].decode()}> {field} 0", ext2)

            image = base / "breenfs.img"

            def build_breenfs():
                self.run_tool(TOOLS / "breenfs-mkfs", "--size", "67108864",
                              "--time", str(TIME), "--populate", source, "--owners", owners, image)
                return self.run_tool(TOOLS / "breenfs-extract", image, "migration-manifest")

            def ext2_manifest(*args):
                return self.run_tool(sys.executable, METAL / "migrate_manifest.py", "ext2", ext2, *args)

            def rows(manifest):
                return dict((fields[0], fields[1:]) for fields in
                            (line.split(b"\t") for line in manifest.splitlines()))

            baseline = ext2_manifest()
            self.assertEqual(build_breenfs(), baseline)
            mapped = rows(baseline)
            self.assertEqual(len(mapped), len(paths))
            program_key = b"/program".hex().encode()
            alias_key = b"/alias".hex().encode()
            child_key = os.fsencode("/nested/" + child.name).hex().encode()
            regular = mapped[program_key]
            self.assertEqual(len(regular), 8)
            self.assertEqual(regular[:3], [b"reg", b"4755", b"1000"])
            self.assertEqual(regular[4], alias_key)
            self.assertEqual(mapped[alias_key], regular)
            self.assertEqual(mapped[b"2f7a65726f"][5], b"8192")
            self.assertEqual(mapped[b"2f7a65726f"][-1], hashlib.sha256(bytes(8192)).hexdigest().encode())
            self.assertEqual(mapped[b"2f73796d"][-1], hashlib.sha256(os.fsencode(target)).hexdigest().encode())
            self.assertEqual(mapped[child_key][-1], hashlib.sha256(b"hello").hexdigest().encode())
            old_manifest = rows(self.run_tool(TOOLS / "breenfs-extract", image, "manifest"))
            self.assertEqual(old_manifest[b"2f7a65726f"][5], b"0", "BreenFS zero file must be a hole")

            subtree = rows(ext2_manifest("--subtree", "/nested"))
            self.assertEqual(subtree, {b"2f": mapped[b"2f6e6573746564"],
                                       os.fsencode("/" + child.name).hex().encode(): mapped[child_key]})

            child.write_bytes(b"jello")
            changed = rows(build_breenfs())
            self.assertEqual(changed[child_key][:-1], mapped[child_key][:-1])
            self.assertEqual(changed[child_key][-1], hashlib.sha256(b"jello").hexdigest().encode())
            self.assertEqual({p for p in mapped if mapped[p] != changed[p]}, {child_key})
            child.write_bytes(b"hello")

            # Change ext2's mode so the BreenFS owners fixture stays constant.
            self.run_tool(debugfs, "-w", "-R", "set_inode_field /program mode 0100755", ext2)
            changed = rows(ext2_manifest())
            for key in (program_key, alias_key):
                expected = mapped[key].copy()
                expected[1] = b"0755"
                self.assertEqual(changed[key], expected)
            self.assertEqual({p for p in mapped if mapped[p] != changed[p]}, {program_key, alias_key})

            # Replace the alias with an identical but independently owned inode.
            alias.unlink()
            alias.write_bytes(b"binary")
            alias.chmod(0o4755)
            with owners.open("ab") as out:
                out.write(f"1000:{program.stat().st_gid}:4755\t/alias\n".encode())
            changed = rows(build_breenfs())
            for key in (program_key, alias_key):
                expected = mapped[key].copy()
                expected[4] = b"-"
                self.assertEqual(changed[key], expected)
            self.assertEqual({p for p in mapped if mapped[p] != changed[p]}, {program_key, alias_key})


if __name__ == "__main__":
    unittest.main()
