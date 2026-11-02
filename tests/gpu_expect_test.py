"""GPU gallery descriptions cover judged pictures and meaningful end screens.

Inventory: suite_gpu.rs case table and its helpers in suite_gpu/*.rs. The two
end-only cases draw/present their scanout but never hand off a judged picture.
Descriptions are documentation; the runner and scorers do not consume them.
"""
from pathlib import Path
import json
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
PICTURED = {
    "device": "no-fallback".split(),
    "isolation": "client-cannot-scanout display-context-unreachable efault-readonly efault-submit foreign-target-draw fork-no-handles fuzz-streams handle-guess handle-private kill-mid-submit killable-wait quota-memory".split(),
    "performance": "clear-rate-2560 coverage-throughput first-draw-latency present-rate-2560 readback-time-2560 rect-throughput upload-bandwidth wait-no-spin".split(),
    "pipelines": "bitmap-text box-filter circle-fill circle-outline clip-nested copy copy-clip copy-scaled coverage-blend coverage-lcd coverage-spans-corpus coverage-spans-square coverage-spans-winding gradient line-bresenham line-clip line-extreme nearest-scale order over-overlap over-premultiplied over-straight rect-batch rounded-rect solid-rects triangle-integer".split(),
    "present": "console-restored fault-skip-flush fault-wrong-scanout flip mode-1280x800 mode-2560x1600 mode-change-content ownership-stack present-completion present-owner-only readback-presented scanout-bind takeover-giveback".split(),
    "profile": "alpha-write binary32-emulation clear-exact cross-context determinism filtered-sampler-refused fixed-blend-off fragcoord-integer int-float-exact integer-ops large-coordinates no-dither no-srgb profile-violation-detected rect-coverage rect-edges scissor-exact swizzle-bgra-rgba texel-fetch-exact unorm-roundtrip viewport-exact wide-arithmetic".split(),
    "readback": "device-path efault error-invalidates-targets fault-corrupt-pixel fault-skip-transfer fault-stale-fence fault-suppress-draw fault-wrong-resource fence-identity format-canonical full-target large-target ordered-after-draws owner-only padded-stride partial-rect poison-unwritten record-fields scenes-aba".split(),
    "submit": "context-create device-error-eio device-reset-recovers fence-echo-checked fence-order fence-wait frame-one-syscall multi-context recovery-after-error ring-backpressure stream-overflow validation-atomic watchdog-quarantine".split(),
    "textures": "copy-overlap copy-region destroy-deferred fault-wrong-texture immutable-while-pinned r8-atlas upload-visibility".split(),
}
END_ONLY = {"isolation/irq-masked-bound", "isolation/no-addresses"}


class GpuExpect(unittest.TestCase):
    def test_pictured_gpu_cases_have_visual_expectations(self):
        pictured = {f"{cat}/{case}" for cat, cases in PICTURED.items() for case in cases}
        self.assertEqual(len(pictured), 121)
        wanted = pictured | END_ONLY
        manifest = json.loads((ROOT / "docs/suites/gpu.json").read_text())
        cases = {f"{cat['id']}/{case['id']}": case
                 for cat in manifest["categories"] for case in cat["cases"]}
        self.assertTrue(wanted <= cases.keys())
        for name, case in cases.items():
            with self.subTest(case=name):
                if name not in wanted:
                    self.assertNotIn("expect", case)
                    continue
                description = case.get("expect")
                self.assertIsInstance(description, str)
                self.assertTrue(description.strip())
                self.assertLessEqual(len(description), 300)
                self.assertRegex(description, r"[A-Za-z][A-Za-z -]* \(#[0-9A-Fa-f]{6}\)")


if __name__ == "__main__":
    unittest.main()
