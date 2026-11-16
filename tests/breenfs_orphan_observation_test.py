"""E24 ledger 309 C5: a removed name alone cannot prove orphan reclamation."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("verdict", ROOT / "scripts/suite-verdict.py")
SCORER = importlib.util.module_from_spec(spec)
spec.loader.exec_module(SCORER)


class OrphanObservationTests(unittest.TestCase):
    def test_guest_records_identity_and_requires_released_data_after_remount(self):
        source = (ROOT / "userspace/programs/src/filesystem_write.rs").read_text()
        case = source.split("pub fn unlinked_open_file()", 1)[1].split("pub fn truncate_sparse", 1)[0]
        self.assertIn('value("orphan-inode",', case)
        self.assertLess(case.index("metadata.ino()"), case.index("drop(held)"))
        self.assertLess(case.index("scratch.remount()?"), case.index("free_after >= free_before + data_blocks"))
        self.assertIn("sysfs::fstatfs", source)

    def test_guest_pass_requires_no_orphan_or_inode_in_stopped_scratch(self):
        manifest = dict(id="filesystem", categories=[dict(id="write", cases=[dict(id="unlinked-open-file")])])
        serial = ("SUITE filesystem START cases=1 only=write/unlinked-open-file\n"
                  "SUITE filesystem VALUE write/unlinked-open-file orphan-inode=16\n"
                  "SUITE filesystem CASE write/unlinked-open-file PASS ms=1\n"
                  "SUITE filesystem DONE passed=1 failed=0 skipped=0 total=1 only=write/unlinked-open-file\n")
        # breenfs-extract dump's wire form; retained inode and ORPHAN are
        # independent defects, even though neither has a directory entry.
        clean = ("superblock generation=8 uuid=" + "73" * 16 + " total_blocks=16384 free_blocks=12000\n"
                 "space object=18446744073709551376 type=90 offset=1 data=00\n"
                 "fs root=1 object=2 type=01 offset=0 data=00\n")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            disk = directory / "ext2-writable.img"
            disk.touch()
            scratch = directory / "breenfs-scratch.img"
            scratch.touch()
            log = directory / "serial.txt"
            log.write_text(serial)

            def score(dump, wire=serial):
                log.write_text(wire)
                def extract(args, **kwargs):
                    self.assertEqual(args[-2:], [str(scratch), "dump"])
                    return subprocess.CompletedProcess(args, 0, stdout=dump.encode())
                with patch.object(SCORER.subprocess, "run", side_effect=extract):
                    return SCORER.verdict(manifest, log, [log], only="write/unlinked-open-file", disk=disk)

            for retained in ("fs root=1 object=16 type=01 offset=0 data=00\n",
                             "fs root=1 object=18446744073709551360 type=40 offset=16 data=\n"):
                with self.subTest(retained=retained):
                    ok, reason = score(clean + retained)
                    self.assertFalse(ok, "guest PASS hid an unreclaimed orphan")
                    self.assertIn("16", reason)
            self.assertTrue(score(clean)[0])
            self.assertFalse(score("")[0])
            self.assertFalse(score(clean, serial.replace("orphan-inode=16", "other=16"))[0])
            scratch.unlink()
            self.assertFalse(score(clean)[0])


if __name__ == "__main__":
    unittest.main()
