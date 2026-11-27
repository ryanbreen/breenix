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


class DownGuest:
    boots = 3

    def alive(self):
        return False


class GuestThatStaysDown(unittest.TestCase):
    def test_every_network_row_counts_the_lost_attempt(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        out = Path(temp.name)
        bench = rig.Rig.__new__(rig.Rig)
        bench.args = type("Args", (), {"run": "rig"})()
        bench.out = {"breenix": out}
        bench.guests = {"breenix": DownGuest()}
        bench.restart = lambda system: None
        bench.sample("network", 1, "breenix")
        groups = rig.collect_receipts(out, ["rig"], "scp-upload")
        for case in ("scp-upload", "download", "ssh"):
            self.assertEqual([sample["error"] for sample in groups[case]],
                             ["breenix VM could not be restarted"], case)


class CryptoOptLevels(unittest.TestCase):
    """The release profiles build libbreenix (the crypto) at opt-level 3, so the
    opt-level z rows need a build that keeps libbreenix at z."""

    def test_each_crypto_row_runs_the_build_its_title_names(self):
        self.assertEqual(rig.probe_binary("aes-gcm-oz"), "bbench-oz")
        self.assertEqual(rig.probe_binary("hmac-sha256-oz"), "bbench-oz")
        self.assertEqual(rig.probe_binary("aes-gcm-o3"), "bbench-o3")
        self.assertEqual(rig.probe_binary("getpid"), "bbench")

    def test_both_workspaces_build_libbreenix_at_z_for_the_oz_rows(self):
        import tomllib
        for manifest in ("userland/Cargo.toml", "userspace/programs/Cargo.toml"):
            profiles = tomllib.loads((rig.ROOT / manifest).read_text())["profile"]
            self.assertEqual(profiles["bench-oz"]["inherits"], "release", manifest)
            self.assertEqual(profiles["bench-oz"]["package"]["libbreenix"]["opt-level"], "z",
                             manifest)
        self.assertIn(rig.BREENIX_OZ / "bbench", rig.BREENIX_ARTIFACTS)


class BuildIdentity(unittest.TestCase):
    """A series names the commit its Breenix binaries were built from, never just HEAD."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.dir = Path(temp.name)
        self.artifacts = [self.dir / "kernel", self.dir / "bbench"]
        for path in self.artifacts:
            path.write_bytes(path.name.encode())
        self.stamp = self.dir / "build.json"

    def test_artifacts_built_at_head_are_accepted(self):
        rig.write_build_stamp(self.stamp, "c0ffee", self.artifacts)
        self.assertEqual(rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts), "c0ffee")

    def test_artifacts_built_at_another_commit_are_refused(self):
        rig.write_build_stamp(self.stamp, "0ld", self.artifacts)
        with self.assertRaisesRegex(ValueError, "built at 0ld"):
            rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts)

    def test_an_artifact_changed_after_the_build_is_refused(self):
        rig.write_build_stamp(self.stamp, "c0ffee", self.artifacts)
        self.artifacts[1].write_bytes(b"rebuilt by hand")
        with self.assertRaisesRegex(ValueError, "bbench"):
            rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts)

    def test_artifacts_with_no_stamp_are_refused(self):
        with self.assertRaisesRegex(ValueError, "build-breenix"):
            rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts)

    def test_a_stamp_missing_an_artifact_is_refused(self):
        rig.write_build_stamp(self.stamp, "c0ffee", self.artifacts[:1])
        with self.assertRaisesRegex(ValueError, "bbench"):
            rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts)


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
