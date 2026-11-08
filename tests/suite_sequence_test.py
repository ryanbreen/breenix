#!/usr/bin/env python3
"""scripts/suite-sequence.py: which suites a boot runs, and how each is scored from one shared serial."""
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SEQUENCE = runpy.run_path(str(ROOT / "scripts/suite-sequence.py"))


def manifest(suite, **fields):
    return json.dumps(dict(id=suite, categories=[dict(id="case", cases=[dict(id="one"), dict(id="two")])], **fields))


def skeleton(suite):
    """A manifest whose every category is planned: no cases, so no binary."""
    return json.dumps(dict(id=suite, categories=[dict(id="later", cases=[])]))


class ResolveTests(unittest.TestCase):
    def test_ui_hold_preserves_metal_aggregate_and_explicit_selection(self):
        expected = "console,directories,processes,signals,time,memory,threads,ipc,terminals,networking,libc,rust-std,utilities,sdk,breengel,files-io".split(",")
        manifest = json.loads((ROOT / "docs/suites/ui-testing.json").read_text())
        self.assertEqual(manifest["aggregate"], "after E22 22.7")
        ids, skipped = SEQUENCE["resolve"]("all", "x86-metal", True)
        self.assertEqual(len(ids), 16)
        self.assertEqual(ids, expected)
        self.assertEqual(dict(skipped)["ui-testing"], "under construction: run --suite ui-testing explicitly")
        self.assertEqual(SEQUENCE["resolve"]("ui-testing", "x86-metal", True)[0], ["ui-testing"])

    def test_window_manager_resolution_on_all_targets(self):
        for target in ("x86-metal", "arm64-qemu", "parallels", "vmware"):
            with self.subTest(target=target):
                ids, skipped = SEQUENCE["resolve"]("all", target, True)
                self.assertNotIn("window-manager", ids)
                self.assertEqual(dict(skipped)["window-manager"], "planned categories have no cases")
                self.assertEqual(SEQUENCE["resolve"]("window-manager", target, True)[0], ["window-manager"])

    def test_metal_all_includes_breengel_at_step_9_4_admission(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/suite-sequence.py"),
                                 "resolve", "all", "--target", "x86-metal"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        suites = result.stdout.strip().split(",")
        self.assertEqual(suites[0], "console")
        self.assertEqual(suites[-1], "files-io")
        self.assertIn("breengel", suites)
        self.assertNotIn("leaving out breengel:", result.stderr)

    def test_metal_named_breengel_remains_runnable_before_admission(self):
        for spec in ("breengel", "console,breengel,files-io"):
            with self.subTest(spec=spec):
                result = subprocess.run([sys.executable, str(ROOT / "scripts/suite-sequence.py"),
                                         "resolve", spec, "--target", "x86-metal"],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), spec)
                self.assertEqual(result.stderr, "")

    def test_arm_all_keeps_breengel(self):
        for target in ("arm64-qemu", "parallels", "vmware"):
            with self.subTest(target=target):
                ids, skipped = SEQUENCE["resolve"]("all", target, True)
                self.assertIn("breengel", ids)
                self.assertNotIn("breengel", dict(skipped))

    def test_metal_all_excludes_gpu_until_native_admission(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/suite-sequence.py"),
                                 "resolve", "all", "--target", "x86-metal"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("console", result.stdout.strip().split(","))
        self.assertNotIn("gpu", result.stdout.strip().split(","))
        self.assertIn("leaving out gpu: native GPU backend is not ADMITTED", result.stderr)
        self.assertIn("--suite gpu", result.stderr)

    def test_metal_named_gpu_remains_runnable_before_admission(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/suite-sequence.py"),
                                 "resolve", "gpu", "--target", "x86-metal"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "gpu")
        self.assertEqual(result.stderr, "")

    def reader(self, manifests, efforts=()):
        files = {f"docs/suites/{suite}.json": text for suite, text in manifests.items()}
        files["docs/efforts/path.json"] = json.dumps(dict(efforts=[dict(id=e, suite=e, targets=["arm64-qemu"]) for e in efforts]))
        return files.get

    def test_host_runners_need_their_persistent_host_path(self):
        read = self.reader({"remote": manifest("remote", hostRunner=True, binary="scripts/metal/server-suite"),
                            "local": manifest("local")}, efforts=("remote", "local"))
        ids, skipped = SEQUENCE["resolve"]("all", "arm64-qemu", read=read)
        self.assertEqual(ids, ["local"])
        self.assertEqual(skipped, [("remote", "host-driven persistent suite")])
        with self.assertRaisesRegex(ValueError, "host-driven"):
            SEQUENCE["resolve"]("remote", "arm64-qemu", read=read)

    def test_all_orders_first_and_last_and_leaves_out_what_the_target_cannot_run(self):
        read = self.reader({"disk": manifest("disk", sequence="last"), "plain": manifest("plain"),
                            "screen": manifest("screen", sequence="first"), "gpu": manifest("gpu", needs3dGpu=True),
                            "later": skeleton("later")},
                           efforts=("disk", "plain", "nomanifest", "gpu", "screen", "later"))
        ids, skipped = SEQUENCE["resolve"]("all", "x86-metal", False, read)
        self.assertEqual(ids, ["screen", "plain", "disk"])
        self.assertEqual(dict(skipped), {"nomanifest": "no manifest", "gpu": "needs a 3D GPU", "later": "no cases yet"})
        ids, _ = SEQUENCE["resolve"]("all", "arm64-qemu", True, read)
        self.assertEqual(ids, ["screen", "plain", "gpu", "disk"])

    def test_all_skips_partially_populated_suites_but_explicit_runs_remain_available(self):
        partial = json.loads(manifest("users"))
        partial["categories"].append(dict(id="later", cases=[]))
        read = self.reader({"plain": manifest("plain"), "users": json.dumps(partial)}, efforts=("plain", "users"))
        ids, skipped = SEQUENCE["resolve"]("all", "x86-metal", False, read)
        self.assertEqual(ids, ["plain"])
        self.assertEqual(skipped, [("users", "planned categories have no cases")])
        self.assertEqual(SEQUENCE["resolve"]("users", "x86-metal", False, read)[0], ["users"])

    def test_all_leaves_out_suites_with_host_run_categories(self):
        mixed = json.loads(manifest("users"))
        mixed["categories"].append(dict(id="operator", hostRunner=True, cases=[dict(id="persistent")]))
        read = self.reader({"plain": manifest("plain"), "users": json.dumps(mixed)}, efforts=("plain", "users"))
        ids, skipped = SEQUENCE["resolve"]("all", "x86-metal", False, read)
        self.assertEqual(ids, ["plain"])
        self.assertEqual(skipped, [("users", "host-run categories need a persistent boot; run it with its milestone selectors")])
        self.assertEqual(SEQUENCE["resolve"]("users", "x86-metal", False, read)[0], ["users"])

    def test_a_list_runs_in_the_given_order_and_is_refused_where_isolation_needs_another_place(self):
        read = self.reader({"disk": manifest("disk", sequence="last"), "plain": manifest("plain"),
                            "screen": manifest("screen", sequence="first"), "gpu": manifest("gpu", needs3dGpu=True),
                            "later": skeleton("later")})
        self.assertEqual(SEQUENCE["resolve"]("screen,plain,disk", "vmware", True, read)[0], ["screen", "plain", "disk"])
        self.assertEqual(SEQUENCE["resolve"]("disk", "vmware", True, read)[0], ["disk"])
        for spec, why in (("plain,screen", "runs first"), ("disk,plain", "runs last"), ("plain,plain", "twice"),
                          ("plain,missing", "no manifest"), ("plain,gpu", "3D GPU"), ("Plain", "not a suite id"),
                          ("later", "no cases yet")):
            with self.subTest(spec=spec), self.assertRaisesRegex(ValueError, why):
                SEQUENCE["resolve"](spec, "x86-metal", False, read)


class ScoreTests(unittest.TestCase):
    def score(self, serial, suites, manifests=None):
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            (repo / "scripts").mkdir()
            shutil.copy(ROOT / "scripts/suite-verdict.py", repo / "scripts")
            source = repo / "libs/libbreenix/src/suite_memory.rs"
            source.parent.mkdir(parents=True)
            shutil.copy(ROOT / source.relative_to(repo), source)
            (repo / "docs/suites").mkdir(parents=True)
            shutil.copy(ROOT / "docs/boot-path.json", repo / "docs")
            for suite in suites:
                (repo / "docs/suites" / f"{suite}.json").write_text(
                    manifests[suite] if manifests and suite in manifests else manifest(suite))
            log = repo / "serial.txt"
            log.write_text(serial)
            return SEQUENCE["score"](suites, log, [log], repo=repo)

    @staticmethod
    def passing(suite):
        owners = "user_stacks=17 user_pages=300 page_tables=13 page_cache=0 kernel_stacks=40 buffers=8 other=2 receipts=0 settle_ms=100"
        return (f"SUITE {suite} START cases=2\nSUITE {suite} MEMORY before free=1000 heap_free_kb=500 {owners}\n"
                f"SUITE {suite} CASE case/one PASS ms=1\nSUITE {suite} CASE case/two PASS ms=1\n"
                f"SUITE {suite} DONE passed=2 failed=0 skipped=0 total=2\n"
                f"SUITE {suite} MEMORY after free=990 heap_free_kb=496 {owners}\n"
                f"SUITE {suite} MEMORY reclaimed=PASS free_before=1000 free_after=990 allowance=48\n"
                f"SUITE_SEQUENCE READY {suite}\n")

    def test_passing_populated_categories_cannot_complete_a_planned_suite(self):
        mixed = json.loads(manifest("users"))
        mixed["categories"].extend(dict(id=f"planned-{i}", cases=[]) for i in range(6))
        ok, rows = self.score(self.passing("users"), ["users"], {"users": json.dumps(mixed)})
        self.assertFalse(ok)
        self.assertEqual(rows[0][1], "FAIL")
        self.assertIn("planned categories have no cases", rows[0][2])
        for i in range(6):
            self.assertIn(f"planned-{i}", rows[0][2])

    def test_each_suite_is_scored_from_its_own_part_of_the_serial(self):
        serial = ("SUITE_SEQUENCE BEGIN suites=a,b,c,d,e baseline=9\n" + self.passing("a")
                  + "SUITE_SEQUENCE RESET b removed=2 changed=0 tcp=0 tcpwait=0\n" + self.passing("b")
                  + "SUITE_SEQUENCE RESET c removed=0 changed=1 paths=/etc/hosts tcp=0 tcpwait=0\n" + self.passing("c")
                  + "SUITE_SEQUENCE RESET d removed=0 changed=0 tcp=0 tcpwait=0\nSUITE d START cases=2\nSUITE d CASE case/one PASS ms=1\n"
                  + "SUITE_SEQUENCE STALLED d cases=1/2\nSUITE_SEQUENCE RESET e removed=0 changed=0 tcp=0 tcpwait=0\n"
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

    def test_a_connection_left_open_by_an_earlier_suite_is_not_isolation(self):
        serial = (self.passing("a") + "SUITE_SEQUENCE RESET b removed=0 changed=0 tcp=0 tcpwait=812 memfree=9kB heapfree=9kB\n"
                  + self.passing("b") + "SUITE_SEQUENCE RESET c removed=0 changed=0 tcp=1 tcpwait=70000 memfree=9kB heapfree=9kB\n"
                  + self.passing("c"))
        ok, rows = self.score(serial, ["a", "b", "c"])
        self.assertFalse(ok)
        self.assertEqual([status for _, status, _ in rows], ["PASS", "PASS", "NOT-ISOLATED"])
        self.assertIn("tcp=1", rows[2][2])

    def test_a_boot_stopped_mid_suite_records_its_partial_cases(self):
        ok, rows = self.score(self.passing("a") + "SUITE_SEQUENCE RESET b removed=0 changed=0 tcp=0 tcpwait=0\nSUITE b START cases=2\nSUITE b CASE case/one PASS ms=1\n", ["a", "b"])
        self.assertFalse(ok)
        self.assertEqual(rows[0][1], "PASS")
        self.assertEqual(rows[1][1], "STALLED")
        self.assertIn("after 1 of its cases", rows[1][2])

    def test_lost_or_incomplete_reset_never_passes(self):
        for reset in ("", "SUITE_SEQUENCE RESET b removed=0 changed=0\n",
                      "SUITE_SEQUENCE RESET b removed=0 changed=0 tcp=unknown FAIL cannot read /proc/net/tcp\n",
                      "SUITE_SEQUENCE RESET b removed=0 changed=0 tcp=0 tcpwait=0"):
            with self.subTest(reset=reset):
                ok, rows = self.score(self.passing("a") + reset + self.passing("b"), ["a", "b"])
                self.assertFalse(ok)
                self.assertEqual(rows[1][1], "NOT-ISOLATED")

    def test_deadlines_restart_at_each_suite(self):
        boundaries = SEQUENCE["boundaries"]
        self.assertEqual(boundaries("SUITE a START cases=2\nSUITE a CASE case/one PASS ms=1\n"), 1)
        self.assertEqual(boundaries("SUITE a START cases=2\nSUITE_SEQUENCE STALLED a cases=0/2\nSUITE b START cases=2\n"), 3)
        self.assertEqual(boundaries("SUITE a START cases=2"), 0)


if __name__ == "__main__":
    unittest.main()
