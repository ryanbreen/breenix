#!/usr/bin/env python3
"""Exercise host starvation attribution at sampling boundaries and across CPUs."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "scorer", Path(__file__).resolve().parents[1] / "scripts/score-sigkill-teardown.py")
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)


def record(points):
    return {"start": {"clk_tck": 100}, "segments": [{
        "guest": {"pid": 1, "vcpus": {"0": "10", "1": "11"},
                  "tsc_offsets": {"0": 0, "1": 0}},
        "samples": [{"tsc": ms * 1_000_000, "steal": [0, 0],
                     "vcpu": {str(cpu): [delay * 1_000_000, 0, cpu, "R"]
                              for cpu, delay in enumerate(delays)}}
                    for ms, delays in points]}]}


def confirms(points, start=95, duration=10):
    return scorer.host_confirms({"rate": "1000000", "tick_period": "5000000",
                                "at": str(start * 1_000_000),
                                "interval": str(duration * 1_000_000),
                                "observer_vcpu": "0", "sender_vcpu": "1"}, record(points))[0]


class HostStallTest(unittest.TestCase):
    def test_stall_can_be_entirely_outside_window(self):
        self.assertFalse(confirms([(90, (0, 0)), (110, (8, 0))]))

    def test_only_unavoidable_inside_stall_counts(self):
        points = [(90, (0, 0)), (110, (16, 0))]
        self.assertTrue(confirms(points))  # At least 6 ms lies inside.
        self.assertEqual(scorer.host_stall_ns(
            record(points), 95_000_000, 105_000_000, ["0", "1"], 1_000_000),
                         (6_000_000, None))

    def test_partial_margins_do_not_accumulate_as_evidence(self):
        self.assertFalse(confirms([(90, (0, 0)), (100, (5, 0)), (110, (10, 0))]))

    def test_simultaneous_stalls_do_not_add_elapsed_time(self):
        points = [(0, (0, 0)), (100, (50, 50))]
        self.assertFalse(confirms(points, start=0, duration=100))
        self.assertEqual(scorer.host_stall_ns(
            record(points), 0, 100_000_000, ["0", "1"], 1_000_000),
                         (50_000_000, None))

    def test_disjoint_sample_intervals_can_add_elapsed_time(self):
        self.assertTrue(confirms(
            [(0, (0, 0)), (50, (50, 0)), (100, (50, 50))], start=0, duration=100))

    def test_sufficient_stall_on_one_cpu_is_still_evidence(self):
        self.assertTrue(confirms([(0, (0, 0)), (100, (96, 0))], start=0, duration=100))


if __name__ == "__main__":
    unittest.main()
