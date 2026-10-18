"""Host GPU-time judgement must fail closed when serial timing is unresolved."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "gpu_time", Path(__file__).resolve().parents[1] / "scripts/parallels/gpu-time.py")
gpu_time = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_time)


class GpuTimeTest(unittest.TestCase):
    def judge(self, end=6.0, perf=7.0, ns=0, idle=0, final=True, done=True):
        records = [{"kind": "vm", "pid": 1, "accel": "highest"}]
        records += [{"kind": "gpu", "t": t, "ns": n}
                    for t, n in [(0.0, 0), (6.0, idle), (8.0, idle + ns)]]
        records += [{"kind": "line", "t": 1.0,
                     "line": "SUITE_IDLE gpu performance begin ms=5000"},
                    {"kind": "line", "t": end,
                     "line": "SUITE_IDLE gpu performance end"}]
        records += [{"kind": "line", "t": perf,
                     "line": f"SUITE gpu CASE performance/case{i} PASS"}
                    for i in range(8)]
        if final:
            records.append({"kind": "line", "t": perf,
                            "line": "SUITE gpu CASE performance/present-rate-2560 PASS"})
        if done:
            records.append({"kind": "line", "t": perf,
                            "line": "SUITE gpu DONE passed=170 failed=0 skipped=0 total=170"})
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "samples.jsonl"
            path.write_text("".join(json.dumps(r) + "\n" for r in records))
            return gpu_time.judge_record(path, 0)

    def test_prefix_cannot_pass_without_final_case_and_done(self):
        self.assertIn("no final", self.judge(ns=400_000_000, final=False))
        self.assertIn("no final", self.judge(ns=400_000_000, done=False))

    def test_waits_for_completed_category_instead_of_accepting_fast_prefix(self):
        records = [{"kind": "vm", "pid": 1, "accel": "highest"}]
        records += [{"kind": "gpu", "t": t, "ns": ns} for t, ns in
                    [(0, 0), (1, 0), (6, 5000), (7, 25000), (8, 25000)]]
        records += [{"kind": "line", "t": t, "line": line} for t, line in [
            (1, "SUITE_IDLE gpu performance begin ms=5000"),
            (6, "SUITE_IDLE gpu performance end"),
            (7, "SUITE gpu CASE performance/clear-rate-2560 PASS")]]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "samples.jsonl"
            path.write_text("".join(json.dumps(r) + "\n" for r in records))

            def finish(_):
                with path.open("a") as out:
                    for record in [
                        {"kind": "gpu", "t": 11, "ns": 25000},
                        {"kind": "line", "t": 11,
                         "line": "SUITE gpu CASE performance/present-rate-2560 PASS"},
                        {"kind": "line", "t": 11,
                         "line": "SUITE gpu DONE passed=170 failed=0 skipped=0 total=170"},
                        {"kind": "gpu", "t": 12, "ns": 25000},
                    ]:
                        out.write(json.dumps(record) + "\n")

            with patch.object(gpu_time.time, "sleep", side_effect=finish) as sleep:
                self.assertIn("rate 4.0x idle", gpu_time.judge_record(path, 5))
                sleep.assert_called_once()

    def test_batched_performance_cannot_zero_the_floor(self):
        self.assertIn("unresolved", self.judge(perf=6.0))

    def test_nonfinite_interval_is_refused(self):
        self.assertIn("not finite", self.judge(perf=float("nan")))

    def test_positive_interval_still_requires_gpu_work(self):
        self.assertIn("does not show", self.judge())

    def test_resolved_interval_with_sufficient_work_is_accepted(self):
        self.assertIsNone(self.judge(ns=400_000_000))

    def test_category_rate_must_reach_ten_times_the_idle_rate(self):
        # Idle runs at idle/6 ns per second and the category at ns/2: 3 * ns / idle times idle.
        self.assertIn("does not show", self.judge(ns=18_000_000, idle=6_000_000))
        self.assertIsNone(self.judge(ns=22_000_000, idle=6_000_000))


if __name__ == "__main__":
    unittest.main()
