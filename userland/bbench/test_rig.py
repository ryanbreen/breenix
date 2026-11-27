"""The ARM64 QEMU rig's sample order and its Linux image pin."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import rig


class AlternatingSchedule(unittest.TestCase):
    def test_samples_alternate_systems_within_every_case(self):
        order = rig.schedule(["getpid", "rustc-version", "copy"], 3, ("linux", "breenix"),
                             skip={"breenix": {"rustc-version"}})
        self.assertEqual(order[:6], [
            ("getpid", 1, "linux"), ("getpid", 1, "breenix"),
            ("getpid", 2, "linux"), ("getpid", 2, "breenix"),
            ("getpid", 3, "linux"), ("getpid", 3, "breenix"),
        ])
        self.assertEqual(order[6:9], [("rustc-version", n, "linux") for n in (1, 2, 3)])
        self.assertEqual([system for case, _, system in order if case == "copy"],
                         ["linux", "breenix"] * 3)
        self.assertEqual(len(order), len(set(order)))

    def test_single_sample_cases_still_run_once_on_each_system(self):
        order = rig.schedule(["fsync"], 1, ("linux", "breenix"), skip={})
        self.assertEqual(order, [("fsync", 1, "linux"), ("fsync", 1, "breenix")])


class Ratios(unittest.TestCase):
    def test_above_one_always_means_breenix_is_slower(self):
        def row(row_id, better, median):
            return {"id": row_id, "title": row_id, "better": better, "unit": "s",
                    "state": "measured" if median else "deferred", "median": median}
        linux = {"rows": [row("a", "lower", 2.0), row("b", "higher", 100.0), row("c", "lower", 1.0)]}
        breenix = {"rows": [row("a", "lower", 5.0), row("b", "higher", 25.0), row("c", "lower", None)]}
        self.assertEqual({r["id"]: r["ratio"] for r in rig.ratios(linux, breenix)},
                         {"a": 2.5, "b": 4.0, "c": None})


class ImagePin(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.cache = Path(temp.name)
        self.good = b"stock cloud image bytes"
        self.pin = {"url": "https://example.invalid/image.img", "file": "image.img",
                    "sha256": hashlib.sha256(self.good).hexdigest(), "bytes": len(self.good)}

    def download(self, payload):
        def fetch(url, destination):
            self.assertEqual(url, self.pin["url"])
            Path(destination).write_bytes(payload)
        return fetch

    def test_a_download_with_the_pinned_digest_is_kept(self):
        path = rig.fetch_image(self.pin, self.cache, self.download(self.good))
        self.assertEqual(path, self.cache / "image.img")
        self.assertEqual(path.read_bytes(), self.good)
        self.assertEqual(sorted(p.name for p in self.cache.iterdir()), ["image.img"])

    def test_a_download_with_another_digest_is_refused_and_removed(self):
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            rig.fetch_image(self.pin, self.cache, self.download(b"tampered image bytes!!"))
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_a_cached_image_is_verified_before_use(self):
        (self.cache / "image.img").write_bytes(b"bitrot image bytes here")
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            rig.fetch_image(self.pin, self.cache, self.download(self.good))

    def test_the_repository_pin_names_a_dated_release_and_a_digest(self):
        pin = json.loads((Path(rig.__file__).with_name("linux-arm64.json")).read_text())
        self.assertRegex(pin["url"], r"/release-\d{8}/")
        self.assertRegex(pin["sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(pin["url"].rsplit("/", 1)[1], pin["file"])


if __name__ == "__main__":
    unittest.main()
