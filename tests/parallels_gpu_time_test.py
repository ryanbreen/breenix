"""Host GPU-time judgement must fail closed when serial timing is unresolved."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "gpu_time", Path(__file__).resolve().parents[1] / "scripts/parallels/gpu-time.py")
gpu_time = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_time)


class GpuTimeTest(unittest.TestCase):
    def judge(self, end=6.0, perf=7.0, ns=0):
        records = [{"kind": "vm", "pid": 1, "accel": "highest"}]
        records += [{"kind": "gpu", "t": t, "ns": n}
                    for t, n in [(0.0, 0), (6.0, 0), (8.0, ns)]]
        records += [{"kind": "line", "t": 1.0,
                     "line": "SUITE_IDLE gpu performance begin ms=5000"},
                    {"kind": "line", "t": end,
                     "line": "SUITE_IDLE gpu performance end"}]
        records += [{"kind": "line", "t": perf,
                     "line": f"SUITE gpu CASE performance/case{i} PASS"}
                    for i in range(9)]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "samples.jsonl"
            path.write_text("".join(json.dumps(r) + "\n" for r in records))
            return gpu_time.judge_record(path, 0)

    def test_batched_performance_cannot_zero_the_floor(self):
        self.assertIn("unresolved", self.judge(perf=6.0))

    def test_nonfinite_interval_is_refused(self):
        self.assertIn("not finite", self.judge(perf=float("nan")))

    def test_positive_interval_still_requires_gpu_work(self):
        self.assertIn("does not show", self.judge())

    def test_resolved_interval_with_sufficient_work_is_accepted(self):
        self.assertIsNone(self.judge(ns=400_000_000))


if __name__ == "__main__":
    unittest.main()
