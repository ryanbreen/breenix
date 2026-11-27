"""The ARM64 QEMU rig's sample order and its Linux image pin."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

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


class MemoryShape(unittest.TestCase):
    """Both guests get the repository's standard memory for the work the rig runs: the
    compile rows need the rustc and cargo suites' declared minimum, and the toolchain
    effort's acceptance (self-host) boots larger still. A smaller guest starves the
    page cache on both systems and hides the gap (DEV-31's same-shape rule)."""

    def test_the_guest_memory_covers_the_toolchain_suites_minimums(self):
        minimums = {suite: json.loads((rig.ROOT / f"docs/suites/{suite}.json").read_text())
                    ["minimumGuestMemory"]["mib"] for suite in ("rustc", "cargo", "self-host")}
        self.assertEqual(rig.MEMORY, f"{max(minimums.values())}M", minimums)

    def test_both_guests_boot_with_that_memory(self):
        for system in ("linux", "breenix"):
            argv = [str(word) for word in rig.Guest(system, Path("/rig"), Path("/disk"),
                                                    headed=False).argv()]
            self.assertEqual(argv[argv.index("-m") + 1], rig.MEMORY, system)


class RecordingGuest:
    """A guest that answers every bbench run with a receipt and keeps the argv."""

    def __init__(self, home):
        self.home = home
        self.boots = 1
        self.argvs = []

    def alive(self):
        return True

    def ssh(self, words, **kwargs):
        words = list(map(str, words))
        self.argvs.append(words)
        sample = next(word for word in words if word.startswith("qemu-vol-"))
        receipt = {"sample": sample, "argv": [], "wall_ns": 1, "metrics": {}, "error": None}
        return rig.subprocess.CompletedProcess(words, 0, json.dumps(receipt).encode(), b"")


def sampled_argv(test, system, case):
    """The guest command line of sample 1 of `case` on `system`."""
    temp = tempfile.TemporaryDirectory()
    test.addCleanup(temp.cleanup)
    bench = rig.Rig.__new__(rig.Rig)
    bench.args = type("Args", (), {"run": "qemu-vol"})()
    bench.out = {system: Path(temp.name)}
    guest = RecordingGuest(rig.LINUX_HOME if system == "linux" else "/etc/breenix/test-ssh/home")
    bench.guests = {system: guest}
    bench.endpoint = type("Endpoint", (), {"server_address": ("127.0.0.1", 9)})()
    bench.sample(case, 1, system)
    [argv] = [argv for argv in guest.argvs if "qemu-vol-" + case + "-1" in argv]
    return argv


class BreenixCompileRows(unittest.TestCase):
    """The compile rows run on Breenix too, with the hosted aarch64 toolchain staged
    into the image as the rustc and cargo suite boots stage it."""
    FIXTURE = "/usr/libexec/breenix/rustc"

    def test_every_compile_row_is_scheduled_on_both_systems(self):
        order = rig.schedule(list(rig.COMPILE), 1, rig.SYSTEMS, rig.SKIP)
        self.assertEqual(sorted(order), sorted((case, 1, system) for case in rig.COMPILE
                                               for system in rig.SYSTEMS))
        self.assertFalse(set(rig.COMPILE) & set(rig.DEFERRED["breenix"]))

    def test_breenix_compiles_with_the_staged_fork_not_the_linux_toolchain(self):
        target = f"{self.FIXTURE}/targets/aarch64-breenix.json"
        for case, program in (("rustc-version", "rustc"), ("hello-object", "rustc"),
                              ("yes-dev-box", "cargo"), ("yes-release", "cargo"),
                              ("cargo-noop", "cargo")):
            argv = sampled_argv(self, "breenix", case)
            text = " ".join(argv)
            self.assertIn(f"{self.FIXTURE}/bin/{program}", argv, case)
            self.assertNotIn(rig.LINUX_HOME, text, case)
            if case != "rustc-version":
                self.assertEqual(argv[argv.index("--target") + 1], target, case)
            if program == "cargo":
                self.assertIn(f"RUSTC={self.FIXTURE}/bin/rustc", argv, case)
                flags = next(word for word in argv if word.startswith("CARGO_ENCODED_RUSTFLAGS="))
                self.assertIn(f"--sysroot={self.FIXTURE}/sysroot", flags.split("\x1f")[0], case)
                self.assertIn(f"-Clinker={self.FIXTURE}/bin/bld", flags.split("\x1f"), case)

    def test_the_fixture_path_is_where_the_suite_staging_puts_it(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("rust_host_stage",
                                                      rig.ROOT / "scripts/rust_host_stage.py")
        stage = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stage)
        self.assertEqual(stage.HOSTED_TOOLCHAIN, self.FIXTURE)


class RootFilesystem(unittest.TestCase):
    """The disk rows run on the filesystems the R240 and the program target: BreenFS on
    Breenix and ext4 on Linux. Each is the guest's root, on one virtio disk of the same
    shape: BreenFS will not take Breenix's boot-time /home slot (fs/breenfs/boot.rs), and
    explicit mounts beside an ext2 root would have to leave that root read-only."""

    def data_dir(self, system, case):
        argv = sampled_argv(self, system, case)
        return argv[argv.index("qemu-vol-" + case + "-1") - 2]

    def test_disk_rows_write_under_the_home_directory_on_the_root(self):
        for system in rig.SYSTEMS:
            home = rig.LINUX_HOME if system == "linux" else "/etc/breenix/test-ssh/home"
            for case in ("sequential", "small-files", "walk", "fsync", "getpid"):
                self.assertEqual(self.data_dir(system, case), f"{home}/bbench/data/qemu-vol",
                                 (system, case))

    def test_both_guests_attach_one_disk_in_the_same_shape(self):
        def drives(system):
            argv = [str(word) for word in rig.Guest(system, Path("/rig"), Path("/root.raw"),
                                                    headed=False).argv()]
            options = [argv[i + 1] for i, word in enumerate(argv) if word == "-drive"]
            return [sorted(option for option in drive.split(",")
                           if not option.startswith(("file=", "id="))) for drive in options]

        self.assertEqual(len(drives("linux")), 1)
        self.assertEqual(drives("linux"), drives("breenix"))

    def test_setup_refuses_a_root_that_is_not_the_named_filesystem(self):
        self.assertIsNone(rig.root_problem("breenix", "none / breenfs rw 0 0\n"
                                                      "none /proc proc rw 0 0\n"))
        self.assertIn("ext2", rig.root_problem("breenix", "none / ext2 rw,nosuid 0 0\n"))
        self.assertIn("read-write", rig.root_problem("breenix", "none / breenfs ro 0 0\n"))
        self.assertIsNone(rig.root_problem("linux", "/dev/vda1 / ext4 rw,relatime 0 0\n"))
        self.assertIn("btrfs", rig.root_problem("linux", "/dev/vda1 / btrfs rw 0 0\n"))

    def test_each_disk_row_records_its_filesystem(self):
        names = {"breenix": "BreenFS", "linux": "ext4"}
        for system, name in names.items():
            temp = tempfile.TemporaryDirectory()
            self.addCleanup(temp.cleanup)
            series_dir = Path(temp.name) / "series"
            bench = rig.Rig.__new__(rig.Rig)
            bench.args = type("Args", (), {"run": "qemu-vol", "pr": None})()
            bench.out = {system: Path(temp.name)}
            bench.notes = {system: []}
            bench.started = rig.datetime(2026, 11, 27, tzinfo=rig.timezone.utc)
            with mock.patch.object(rig, "SERIES_DIR", series_dir), \
                    mock.patch.object(rig, "qemu_version", lambda: "11.0.3"), \
                    mock.patch.object(rig, "host_description", lambda: "Mac"):
                path = bench.write(system, "0" * 40, {}, {}, {"kernel": "Linux 6.8.0"})
            rows = json.loads(Path(path).read_text())["rows"]
            for row in rows:
                if row["area"] == "disk":
                    self.assertIn(name, row["cache"], (system, row["id"]))
                else:
                    self.assertNotIn(name, row["cache"], (system, row["id"]))


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


class RunIds(unittest.TestCase):
    """A run ID the series writer would refuse is refused before anything boots."""

    def refused(self, run):
        reached = mock.Mock(side_effect=AssertionError(f"{run!r} reached the run"))
        with mock.patch.object(rig, "git", lambda *args: ""), \
                mock.patch.object(rig, "check_breenix_inputs", reached), \
                mock.patch.object(rig, "provision_linux", reached), \
                mock.patch("sys.stderr"):
            with self.assertRaises(SystemExit) as stopped:
                rig.cli(["run", "--run", run])
        self.assertEqual(stopped.exception.code, 2, run)

    def test_a_host_name_in_the_id_is_refused(self):
        # The note names the sidecar by the series ID, and Vigil refuses host names.
        self.refused("localhost-baseline")

    def test_an_id_too_long_for_the_breenix_series_is_refused(self):
        # 113 + "-linux" fits Vigil's 120 characters; 113 + "-breenix" does not.
        self.refused("a" * 113)


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
