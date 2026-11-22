"""Exercise ARM64 cut orchestration with a stand-in process and real BreenFS images."""

import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

from metal_breenfs_replay_test import ROOT, TOOLS, START, ack, record


class QemuPowerCutTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.serial = self.directory / "writer.txt"
        self.image = self.directory / "breenfs-writable.img"
        self.fake = self.directory / "fake-vm.py"
        self.fake.write_text("""
import json
from pathlib import Path
import sys
import time

root = Path(__file__).parent
args = sys.argv[1:]
drive = args[args.index('-drive') + 1]
serial_config = args[args.index('-chardev') + 1]
serial = Path(serial_config.split('logfile=', 1)[1])
case = next(arg for arg in args if arg.startswith('name=opt/breenix/case,')).split('string=', 1)[1]
with (root / 'launches').open('a') as log:
    print(json.dumps(dict(drive=drive, serial=str(serial), case=case, args=args)), file=log)
if '-pidfile' in args:
    import os
    Path(args[args.index('-pidfile') + 1]).write_text(str(os.getpid()))
kind = 'recovery' if case.endswith('/recovery') else 'writer'
serial.write_bytes((root / kind).read_bytes())
if (root / 'exit-early').exists():
    sys.exit(0)
time.sleep(20)
""")

    def make_image(self):
        tree = self.directory / "tree"
        (tree / "crash").mkdir(parents=True)
        (tree / "crash/log").write_bytes(record(1))
        subprocess.run([TOOLS / "breenfs-mkfs", "--size", str(64 << 20),
                        "--populate", tree, self.image], check=True, capture_output=True)

    def run_cut(self, *options):
        return subprocess.run(
            [sys.executable, ROOT / "scripts/qemu-power-cut.py", "--serial", self.serial,
             "--disk", self.image, "--tools-dir", TOOLS, "--gate-timeout", "4", *options,
             "--", sys.executable, self.fake],
            capture_output=True, text=True, timeout=12,
        )

    def recovery_capture(self):
        digest = hashlib.sha256(record(1)).hexdigest()
        return (f"BREENFS POWER-CUT RECOVERY root-is-breenfs bytes=4096 sha256={digest}\n"
                "SUITE filesystem DONE passed=1 failed=0 skipped=0 total=1 only=power-cut/recovery\n"
                "SUITE_SEQUENCE READY filesystem\n")

    def test_aimed_cut_replays_and_recovers_the_same_raw_disk(self):
        self.make_image()
        (self.directory / "writer").write_bytes(
            START + ack(1) + b"[breenfs] commit-window first-flush\r\n")
        (self.directory / "recovery").write_text(self.recovery_capture())
        result = self.run_cut("--power-cut-window", "first-flush")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        launches = [json.loads(line) for line in (self.directory / "launches").read_text().splitlines()]
        self.assertEqual([launch["case"] for launch in launches], ["power-cut/writer", "power-cut/recovery"])
        for launch in launches:
            self.assertEqual(launch["drive"],
                             f"if=none,id=ext2,format=raw,file={self.image},cache=writeback,aio=threads")
        self.assertNotEqual(launches[0]["serial"], launches[1]["serial"])
        self.assertTrue(any("name=opt/breenix/intent,file=" in arg for arg in launches[0]["args"]))
        self.assertFalse(any("name=opt/breenix/intent" in arg for arg in launches[1]["args"]))
        cut = json.loads((self.directory / "run.json").read_text())["powerCut"]
        self.assertEqual(cut["qemuExit"], -signal.SIGKILL)
        cold = json.loads((self.directory / "cut-readback.json").read_text())
        self.assertEqual((cold["coldState"], cold["records"], cold["acknowledged"]), ("clean", 1, 1))
        self.assertTrue(json.loads((self.directory / "cut-cycle.json").read_text())["passed"])

    def test_timed_cut_with_lost_ack_stops_before_recovery(self):
        self.make_image()
        (self.directory / "writer").write_bytes(START + ack(1) + ack(2))
        result = self.run_cut("--power-cut-after", "0.5")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("acknowledged sequence 2 is missing", result.stderr)
        self.assertEqual(len((self.directory / "launches").read_text().splitlines()), 1)

    def test_unexpected_qemu_exit_cannot_count_as_a_cut(self):
        self.image.touch()
        (self.directory / "writer").write_bytes(START + ack(1))
        (self.directory / "exit-early").touch()
        result = self.run_cut("--power-cut-after", "0.5")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exited before the cut trigger", result.stderr)
        self.assertFalse((self.directory / "cut-readback.json").exists())

    def test_bit_flip_requires_the_typed_read_and_cold_checker(self):
        self.make_image()
        import mmap
        with self.image.open("r+b") as disk, mmap.mmap(disk.fileno(), 0) as mapped:
            offset = mapped.find(record(1))
            self.assertGreater(offset, 0)
            self.assertEqual(offset % 4096, 0)
            mapped[offset + 64] ^= 1
        block = offset // 4096
        (self.directory / "writer").write_bytes(
            START + ack(1) + f"[breenfs] corrupt-inject block={block} offset=64 bit=0\n"
            f"[breenfs] corrupt-read block={block} result=ReadError::Data\n"
            "SUITE filesystem DONE passed=1 failed=0 skipped=0 total=1 only=power-cut/writer\n"
            "SUITE_SEQUENCE READY filesystem\n".encode())
        result = self.run_cut("--breenfs-bit-flip")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len((self.directory / "launches").read_text().splitlines()), 1)
        cold = json.loads((self.directory / "bit-flip-readback.json").read_text())
        self.assertEqual((cold["block"], cold["breenfsCheckExit"]), (block, 4))

    def test_launcher_requires_a_disposable_breenfs_writer_root(self):
        result = subprocess.run(
            [ROOT / "scripts/boot-interactive.sh", "--build-only", "--mode", "suite", "--suite", "filesystem",
             "--case", "power-cut/writer", "--power-cut-window", "node-writes"],
            env=dict(os.environ, BREENIX_SLOT_SESSION="host-check"), text=True, capture_output=True, timeout=5,
        )
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("need --mode suite --suite filesystem --case power-cut/writer --root-fs breenfs",
                      result.stderr)


if __name__ == "__main__":
    unittest.main()
