"""GPU and BreenGL gallery descriptions cover judged pictures and meaningful end screens.

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


# BreenGL handoffs: raster.rs, images.rs, widgets.rs, show.rs and window_cases/.
# End-only cases compare/present a scanout without handing a picture to the stage.
BREENGL_PICTURED = {
    "bmp": "decode-fixtures orientation padding roundtrip truncated".split(),
    "composition": "compose-position compose-overlap compose-offscreen compose-fullscreen compose-many no-compositor backend-error cursor-hidden cursor-no-trail cursor-shape cursor-stationary-repaint takeover-giveback ownership-stack gpu-composite scale-2 target-lifetime".split(),
    "events": "resize-request processed-ack".split(),
    "icons": "step-invariance settles reset bounded particles-bounded assets".split(),
    "isolation": "list-by-user other-commit other-map other-resize other-destroy compositor-authorized efault-commit".split(),
    "panel": "stage-layout tile-stamps tile-thumbnail spotlight-picture spotlight-timing milestone-ribbon finale deterministic stage-hidpi handoff between-cases cursor-restore cursor-trail-detected kernel-screens".split(),
    "png-decode": "gray-depths gray-alpha rgb rgba palette-depths trns-gray trns-rgb trns-palette rounding-16bit filters interlace interlace-small multi-idat chunk-order ancillary-ignored independent-fixtures blit from-disk".split(),
    "png-encode": "decodes stable sizes".split(),
    "raster": "fill-rect fill-rect-clip-left fill-rect-offscreen-left fill-rect-offscreen-top fill-rect-overflow fill-rect-empty draw-rect line-axes line-bresenham line-clip circle-fill circle-fill-offscreen-left circle-outline extreme-work-bound bgr-order x-byte alpha-extremes alpha-output blit blit-clip blit-alpha blit-straight blit-scale clip-rect clip-stack clip-empty rounded-rect gradient damage-sound".split(),
    "scanout": "changing-scene full-destination binding atomic-record format-canonical fault-skipped-transfer fault-wrong-resource fault-stale-fence fault-outside-damage present-flip hidpi-mode".split(),
    "text": "bitmap-glyph bitmap-scale bitmap-unknown noto-ascii noto-coverage ttf-glyph ttf-sizes ttf-clip-left ttf-clip-top ttf-subpixel ttf-notdef utf8-text utf8-invalid compound-transform many-args-abi hidpi-sizes fontwatch-reload fontwatch-size fontwatch-missing fontwatch-malformed-config fontwatch-repeated fontwatch-independent fontwatch-idle fontwatch-file-replaced".split(),
    "text-entry": "scroll-to-cursor filepicker-errors".split(),
    "widgets": "button-draw button-disabled checkbox-toggle label-clip tabbar-overflow scrollbar-thumb menubar-open menu-disabled focus-ring scale-2".split(),
    "windows": "create create-zeroed multi-window destroy fork-no-mapping commit-swaps acquire-replays-damage acquire-full-copy stale-base-commit commit-immutable release-order resize capture-across-destroy remnant-on-death raw-client commit-waits-render".split(),
}
BREENGL_END_ONLY = {
    "composition/composite-record",
    "composition/damage-screen",
    "composition/damage-work",
    "composition/fbinfo-geometry",
    "composition/inflight-bound",
    "composition/irq-bound",
    "composition/no-change-commit",
    "composition/present-rate",
    "isolation/texture-map",
    "scanout/boundary-stated",
    "scanout/completion",
    "scanout/device-path",
    "scanout/mode-set",
    "scanout/root-only",
    "scanout/window-commits",
    "windows/distinct-content",
}


class GpuExpect(unittest.TestCase):
    def test_reviewed_colour_samples(self):
        def descriptions(suite):
            manifest = json.loads((ROOT / f"docs/suites/{suite}.json").read_text())
            return {f"{cat['id']}/{case['id']}": case.get("expect", "")
                    for cat in manifest["categories"] for case in cat["cases"]}

        breengel = descriptions("breengel")
        for text in ("1-bit", "#555555", "#777777", "#7F7F7F"):
            self.assertIn(text, breengel["png-decode/gray-depths"])
        self.assertIn("black, black", breengel["png-decode/trns-gray"])
        self.assertIn("dark indigo (#1B0F5F)", breengel["isolation/other-map"])
        self.assertIn("crimson (#7B2D43) probe pixel at (13,1)", breengel["raster/bgr-order"])

        gpu = descriptions("gpu")
        for case, sample in (("textures/copy-overlap", "#6FF08F"),
                             ("textures/destroy-deferred", "#86FFE8"),
                             ("pipelines/coverage-lcd", "#312719"),
                             ("performance/wait-no-spin", "#AC7625")):
            with self.subTest(case=case):
                self.assertIn(sample, gpu[case])
        self.assertIn("#011D0D", breengel["windows/release-order"])

    def test_end_screen_descriptions_fit_the_exporter(self):
        # tools/screen-gallery/export.py refuses an expectEnd over 300 characters.
        for suite in ("gpu", "breengel"):
            manifest = json.loads((ROOT / f"docs/suites/{suite}.json").read_text())
            for cat in manifest["categories"]:
                for case in cat["cases"]:
                    if "expectEnd" not in case:
                        continue
                    with self.subTest(case=f"{suite}/{cat['id']}/{case['id']}"):
                        self.assertTrue(case["expectEnd"].strip())
                        self.assertLessEqual(len(case["expectEnd"]), 300)

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

    def test_pictured_breengel_cases_have_visual_expectations(self):
        pictured = {f"{cat}/{case}" for cat, cases in BREENGL_PICTURED.items() for case in cases}
        self.assertEqual(len(pictured), 163)
        wanted = pictured | BREENGL_END_ONLY
        manifest = json.loads((ROOT / "docs/suites/breengel.json").read_text())
        cases = {f"{cat['id']}/{case['id']}": case
                 for cat in manifest["categories"] for case in cat["cases"]}
        self.assertEqual(len(cases), 401)
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
