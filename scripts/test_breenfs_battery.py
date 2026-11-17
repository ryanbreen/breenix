#!/usr/bin/env python3
"""Tests for scripts/breenfs-battery.py.

The battery's stages are replaced by a stub (breenfs_battery_fixtures/libtest_stub.py)
that prints canned libtest output, so no cargo build, host test or breenfs-torture
seed runs here. Run: python3 scripts/test_breenfs_battery.py
"""

import fcntl
import importlib.util
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
STUB = HERE / "breenfs_battery_fixtures" / "libtest_stub.py"

spec = importlib.util.spec_from_file_location("breenfs_battery", HERE / "breenfs-battery.py")
battery = importlib.util.module_from_spec(spec)
sys.modules["breenfs_battery"] = battery
spec.loader.exec_module(battery)

AT = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
EVENT_FIELDS = {
    "stage-start": {"at", "event", "stage", "planned"},
    "case": {"at", "event", "stage", "name", "outcome"},
    "progress": {"at", "event", "stage", "done", "planned", "failures"},
    "stage-finish": {"at", "event", "stage", "outcome", "seconds", "done", "failures", "summary"},
    "note": {"at", "event", "text"},
}
OPTIONAL_FIELDS = {"case": {"seconds", "message"}, "progress": {"images", "current", "seeds"}}


def stub_stage(stage_id: str, scenario: str, *extra: str):
    return battery.LibtestStage(
        id=stage_id, title=f"Stub {stage_id}", checks="Stub output parsed as libtest.",
        command="stub", unit="tests", planned=None, known_issues=battery.KNOWN_ISSUES,
        list_argv=[sys.executable, str(STUB), "list"],
        run_argv=[sys.executable, str(STUB), "run", scenario, *extra],
    )


def read_events(run_dir: Path) -> list[dict]:
    return [json.loads(line) for line in (run_dir / "events.jsonl").read_text().splitlines()]


def only_run(root: Path) -> Path:
    runs = [p for p in root.iterdir() if p.is_dir()]
    assert len(runs) == 1, runs
    return runs[0]


class BatteryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "batteries"

    def tearDown(self):
        self.temp.cleanup()

    def run_battery(self, *stages) -> tuple[int, Path]:
        code = battery.Battery(self.root, list(stages), "full", 8).run()
        return code, only_run(self.root)

    def test_known_issue_is_known_and_a_real_failure_fails(self):
        code, run_dir = self.run_battery(stub_stage("host-corpus", "known"), stub_stage("vfs", "real"))
        self.assertEqual(code, 1)
        result = json.loads((run_dir / "result.json").read_text())
        self.assertEqual(result["outcome"], "fail")
        self.assertEqual([(s["id"], s["outcome"], s["done"], s["failures"]) for s in result["stages"]],
                         [("host-corpus", "known", 3, 1), ("vfs", "fail", 3, 1)])
        cases = {(e["stage"], e["name"]): e for e in read_events(run_dir) if e["event"] == "case"}
        known = cases[("host-corpus", "torn_images_recover_to_an_acknowledged_commit")]
        self.assertEqual((known["outcome"], known["message"], known["seconds"]),
                         ("known", "torn corpus exceeded 10 minutes", 601.25))
        real = cases[("vfs", "torn_images_recover_to_an_acknowledged_commit")]
        self.assertEqual(real["outcome"], "fail")
        self.assertTrue(real["message"].startswith("assertion `left == right` failed: seed=2"))
        self.assertEqual(cases[("vfs", "a::two")]["outcome"], "ignored")
        finishes = [e for e in read_events(run_dir) if e["event"] == "stage-finish"]
        self.assertEqual(finishes[0]["summary"], "1 passed, 1 known (#3023), 1 ignored")
        log = (run_dir / "stages" / "host-corpus.log").read_text()
        self.assertIn("torn corpus exceeded 10 minutes", log)

    def test_records_match_the_contract(self):
        code, run_dir = self.run_battery(stub_stage("host-corpus", "pass"))
        self.assertEqual(code, 0)
        run = json.loads((run_dir / "run.json").read_text())
        self.assertEqual(set(run), {"schema", "id", "battery", "commit", "branch", "dirty", "startedAt",
                                    "profile", "tortureSeeds", "pid", "stages"})
        self.assertRegex(run["id"], r"^\d{8}T\d{6}Z-breenfs$")
        self.assertEqual(run_dir.name, run["id"])
        self.assertRegex(run["commit"], r"^[0-9a-f]{40}$")
        self.assertRegex(run["startedAt"], AT)
        self.assertEqual((run["schema"], run["battery"], run["pid"]), (1, "breenfs", os.getpid()))
        self.assertEqual(set(run["stages"][0]), {"id", "title", "checks", "command", "unit", "planned",
                                                 "knownIssues"})
        self.assertEqual(run["stages"][0]["knownIssues"][0],
                         {"test": "torn_images_recover_to_an_acknowledged_commit", "issue": "#3023"})
        events = read_events(run_dir)
        for event in events:
            kind = event["event"]
            self.assertRegex(event["at"], AT)
            self.assertLessEqual(EVENT_FIELDS[kind], set(event), event)
            self.assertLessEqual(set(event), EVENT_FIELDS[kind] | OPTIONAL_FIELDS.get(kind, set()), event)
        kinds = [e["event"] for e in events]
        self.assertEqual(kinds, ["note", "stage-start", "case", "case", "case", "stage-finish"])
        self.assertEqual(events[1]["planned"], 3)
        self.assertEqual([(e["name"], e["outcome"], e.get("seconds")) for e in events[2:5]],
                         [("a::one", "pass", 0.012), ("a::two", "ignored", None),
                          ("torn_images_recover_to_an_acknowledged_commit", "pass", 12.5)])
        self.assertEqual((events[5]["outcome"], events[5]["done"], events[5]["failures"]), ("pass", 3, 0))
        result = json.loads((run_dir / "result.json").read_text())
        self.assertEqual(set(result), {"finishedAt", "outcome", "exit", "stages"})
        self.assertEqual((result["outcome"], result["exit"]), ("pass", 0))
        self.assertEqual(set(result["stages"][0]), {"id", "outcome", "seconds", "done", "failures"})
        self.assertFalse(list(run_dir.glob("*.tmp")))

    def test_torture_seeds_become_progress_events(self):
        torture = HERE / "breenfs_battery_fixtures" / "torture_stub.py"
        stage = battery.TortureStage(
            id="torture", title="Stub torture", checks="Stub seeds.", command="stub", unit="seeds",
            planned=5, seeds=5, builds=[], torture=lambda: torture, checker=lambda: Path("checker"),
        )
        code, run_dir = self.run_battery(stage)
        self.assertEqual(code, 1)
        events = read_events(run_dir)
        progress = [e for e in events if e["event"] == "progress"]
        last = progress[-1]
        self.assertEqual((last["done"], last["planned"], last["failures"], last["images"]), (5, 5, 1, 1500))
        self.assertEqual([(s["seed"], s["state"], s["images"]) for s in last["seeds"]],
                         [(1, "pass", 100), (2, "pass", 200), (3, "fail", 300), (4, "pass", 400),
                          (5, "pass", 500)])
        finish = [e for e in events if e["event"] == "stage-finish"][0]
        self.assertEqual((finish["outcome"], finish["done"], finish["failures"]), ("fail", 5, 1))
        self.assertEqual(finish["summary"], "4 of 5 seeds passed, 1,500 images checked")
        self.assertTrue((run_dir / "stages" / "torture" / "seed-3" / "progress.json").exists())
        self.assertIn("[seed 3] seed=3", (run_dir / "stages" / "torture.log").read_text())

    def drive(self, *args: str, **kwargs) -> subprocess.Popen:
        return subprocess.Popen([sys.executable, __file__, "--drive", str(self.root), *args],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, **kwargs)

    def test_sigterm_stops_the_stage_and_writes_a_stopped_result(self):
        pidfile = Path(self.temp.name) / "stub.pid"
        driver = self.drive("hang", str(pidfile))
        deadline = time.monotonic() + 30
        while not pidfile.exists() or not pidfile.read_text():
            self.assertLess(time.monotonic(), deadline, "the stub stage never started")
            if driver.poll() is not None:
                self.fail(f"the battery exited early: {driver.communicate()}")
            time.sleep(0.1)
        stub_pid = int(pidfile.read_text())
        driver.send_signal(signal.SIGTERM)
        _, err = driver.communicate(timeout=30)
        self.assertEqual(driver.returncode, 143, err)
        run_dir = only_run(self.root)
        result = json.loads((run_dir / "result.json").read_text())
        self.assertEqual(result["outcome"], "stopped")
        self.assertEqual([(s["id"], s["outcome"]) for s in result["stages"]],
                         [("host-corpus", "stopped"), ("vfs", "stopped")])
        finishes = [e for e in read_events(run_dir) if e["event"] == "stage-finish"]
        self.assertEqual([(e["stage"], e["outcome"]) for e in finishes], [("host-corpus", "stopped")])
        with self.assertRaises(ProcessLookupError):
            os.kill(stub_pid, 0)

    def test_a_held_lock_refuses_a_second_run(self):
        self.root.mkdir(parents=True)
        with open(self.root / ".lock", "a") as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            driver = self.drive("pass")
            _, err = driver.communicate(timeout=30)
        self.assertEqual(driver.returncode, 75)
        self.assertIn("another battery is running", err)
        self.assertEqual([p for p in self.root.iterdir() if p.is_dir()], [])


def drive(root: str, scenario: str, *extra: str) -> int:
    stages = [stub_stage("host-corpus", scenario, *extra), stub_stage("vfs", "pass")]
    run = battery.Battery(Path(root), stages, "full", 8)
    signal.signal(signal.SIGTERM, run.request_stop)
    return run.run()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--drive":
        sys.exit(drive(*sys.argv[2:]))
    unittest.main()
