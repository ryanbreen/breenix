#!/usr/bin/env python3
"""scripts/suite-sequence.py: which suites a boot runs, and how each is scored from one shared serial."""
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SEQUENCE = runpy.run_path(str(ROOT / "scripts/suite-sequence.py"))


def manifest(suite, **fields):
    return json.dumps(dict(id=suite, categories=[dict(id="case", cases=[dict(id="one"), dict(id="two")])], **fields))


class ResolveTests(unittest.TestCase):
    def reader(self, manifests, efforts=()):
        files = {f"docs/suites/{suite}.json": text for suite, text in manifests.items()}
        files["docs/efforts/path.json"] = json.dumps(dict(efforts=[dict(id=e, suite=e, targets=["arm64-qemu"]) for e in efforts]))
        return files.get

    def test_all_orders_first_and_last_and_leaves_out_what_the_target_cannot_run(self):
        read = self.reader({"disk": manifest("disk", sequence="last"), "plain": manifest("plain"),
                            "screen": manifest("screen", sequence="first"), "gpu": manifest("gpu", needs3dGpu=True)},
                           efforts=("disk", "plain", "nomanifest", "gpu", "screen"))
        ids, skipped = SEQUENCE["resolve"]("all", "x86-metal", False, read)
        self.assertEqual(ids, ["screen", "plain", "disk"])
        self.assertEqual(dict(skipped), {"nomanifest": "no manifest", "gpu": "needs a 3D GPU"})
        ids, _ = SEQUENCE["resolve"]("all", "arm64-qemu", True, read)
        self.assertEqual(ids, ["screen", "plain", "gpu", "disk"])

    def test_a_list_runs_in_the_given_order_and_is_refused_where_isolation_needs_another_place(self):
        read = self.reader({"disk": manifest("disk", sequence="last"), "plain": manifest("plain"),
                            "screen": manifest("screen", sequence="first"), "gpu": manifest("gpu", needs3dGpu=True)})
        self.assertEqual(SEQUENCE["resolve"]("screen,plain,disk", "vmware", True, read)[0], ["screen", "plain", "disk"])
        self.assertEqual(SEQUENCE["resolve"]("disk", "vmware", True, read)[0], ["disk"])
        for spec, why in (("plain,screen", "runs first"), ("disk,plain", "runs last"), ("plain,plain", "twice"),
                          ("plain,missing", "no manifest"), ("plain,gpu", "3D GPU"), ("Plain", "not a suite id")):
            with self.subTest(spec=spec), self.assertRaisesRegex(ValueError, why):
                SEQUENCE["resolve"](spec, "x86-metal", False, read)


class ScoreTests(unittest.TestCase):
    def score(self, serial, suites):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            (repo / "scripts").mkdir()
            shutil.copy(ROOT / "scripts/suite-verdict.py", repo / "scripts")
            (repo / "docs/suites").mkdir(parents=True)
            shutil.copy(ROOT / "docs/boot-path.json", repo / "docs")
            for suite in suites:
                (repo / "docs/suites" / f"{suite}.json").write_text(manifest(suite))
            log = repo / "serial.txt"
            log.write_text(serial)
            return SEQUENCE["score"](suites, log, [log], repo=repo)

    @staticmethod
    def passing(suite):
        return (f"SUITE {suite} START cases=2\nSUITE {suite} CASE case/one PASS ms=1\nSUITE {suite} CASE case/two PASS ms=1\n"
                f"SUITE {suite} DONE passed=2 failed=0 skipped=0 total=2\nSUITE_SEQUENCE READY {suite}\n")

    def test_each_suite_is_scored_from_its_own_part_of_the_serial(self):
        serial = ("SUITE_SEQUENCE BEGIN suites=a,b,c,d,e baseline=9\n" + self.passing("a")
                  + "SUITE_SEQUENCE RESET b removed=2 changed=0\n" + self.passing("b")
                  + "SUITE_SEQUENCE RESET c removed=0 changed=1 paths=/etc/hosts\n" + self.passing("c")
                  + "SUITE_SEQUENCE RESET d removed=0 changed=0\nSUITE d START cases=2\nSUITE d CASE case/one PASS ms=1\n"
                  + "SUITE_SEQUENCE STALLED d cases=1/2\nSUITE_SEQUENCE RESET e removed=0 changed=0\n"
                  + "SUITE e START cases=2\nSUITE e CASE case/one PASS ms=1\nKERNEL PANIC: in e\n")
        ok, rows = self.score(serial, ["a", "b", "c", "d", "e", "f"])
        self.assertFalse(ok)
        status = {suite: (state, detail) for suite, state, detail in rows}
        self.assertEqual(status["a"][0], "PASS")
        self.assertEqual(status["b"][0], "PASS")
        self.assertEqual(status["c"][0], "NOT-ISOLATED")
        self.assertIn("/etc/hosts", status["c"][1])
        self.assertIn("PASS", status["c"][1])
        self.assertEqual(status["d"], ("STALLED", "the runner's watchdog stopped it after 1 of 2 cases"))
        self.assertEqual(status["e"][0], "FAIL")
        self.assertIn("in e", status["e"][1])
        self.assertEqual(status["f"][0], "NOT-RUN")

    def test_a_boot_stopped_mid_suite_records_its_partial_cases(self):
        ok, rows = self.score(self.passing("a") + "SUITE b START cases=2\nSUITE b CASE case/one PASS ms=1\n", ["a", "b"])
        self.assertFalse(ok)
        self.assertEqual(rows[0][1], "PASS")
        self.assertEqual(rows[1][1], "STALLED")
        self.assertIn("after 1 of its cases", rows[1][2])

    def test_deadlines_restart_at_each_suite(self):
        boundaries = SEQUENCE["boundaries"]
        self.assertEqual(boundaries("SUITE a START cases=2\nSUITE a CASE case/one PASS ms=1\n"), 1)
        self.assertEqual(boundaries("SUITE a START cases=2\nSUITE_SEQUENCE STALLED a cases=0/2\nSUITE b START cases=2\n"), 3)
        self.assertEqual(boundaries("SUITE a START cases=2"), 0)


class StagingTests(unittest.TestCase):
    def test_metal_staging_checks_each_sequence_binary_is_an_elf(self):
        bindir = Path("/opt/homebrew/opt/e2fsprogs/sbin")
        path = str(bindir) + ":" + os.environ["PATH"]
        mkfs, debugfs = shutil.which("mke2fs", path=path), shutil.which("debugfs", path=path)
        if not mkfs or not debugfs:
            self.skipTest("requires e2fsprogs")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            image = root / "disk.img"
            image.write_bytes(bytes(8 * 1024 * 1024))
            subprocess.run([mkfs, "-t", "ext2", "-F", str(image)], check=True, capture_output=True)
            (root / "elf").write_bytes(b"\x7fELF" + bytes(60))
            (root / "commands").write_text(f"mkdir /etc\nmkdir /sbin\nwrite {root}/elf /sbin/suite-first\nwrite {root}/elf /sbin/suite-second\n")
            subprocess.run([debugfs, "-w", "-f", str(root / "commands"), str(image)], check=True, capture_output=True)
            script = str(ROOT / "scripts/write-boot-target.sh")
            env = dict(os.environ, PATH=path)
            staged = subprocess.run([script, str(image), "first", "", "first,second"], env=env, capture_output=True)
            self.assertEqual(staged.returncode, 0, staged.stderr)
            listed = subprocess.run([debugfs, "-R", "cat /etc/breenix/suite-sequence", str(image)], capture_output=True)
            self.assertEqual(listed.stdout, b"first,second\n")
            missing = subprocess.run([script, str(image), "first", "", "first,third"], env=env, capture_output=True)
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn(b"no /sbin/suite-third", missing.stderr)


if __name__ == "__main__":
    unittest.main()
