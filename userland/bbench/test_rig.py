"""The ARM64 QEMU rig's sample order and its Linux image pin."""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import threading
import time
from types import SimpleNamespace
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

    @staticmethod
    def expected():
        """The manifests' largest minimum, read here and not from the rig."""
        minimums = {suite: json.loads((rig.ROOT / f"docs/suites/{suite}.json").read_text())
                    ["minimumGuestMemory"]["mib"] for suite in ("rustc", "cargo", "self-host")}
        return f"{max(minimums.values())}M", minimums

    def test_the_guest_memory_covers_the_toolchain_suites_minimums(self):
        expected, minimums = self.expected()
        self.assertEqual(rig.MEMORY, expected, minimums)

    def test_both_guests_boot_with_that_memory(self):
        # Against the manifests, not rig.MEMORY: the launch and the constant could be
        # wrong together.
        expected, minimums = self.expected()
        for system in ("linux", "breenix"):
            argv = [str(word) for word in rig.Guest(system, Path("/rig"), Path("/disk"),
                                                    headed=False).argv()]
            self.assertEqual(argv[argv.index("-m") + 1], expected, (system, minimums))


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

    def test_the_rig_stages_the_toolchain_the_compile_rows_run(self):
        """The image the rig boots is staged by rust_host_stage.py for the rustc and cargo
        suites on aarch64, which puts the hosted toolchain (its rustc selection) and
        Cargo (its cargo selection) at HOSTED_TOOLCHAIN, the path the rows run."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("rust_host_stage",
                                                      rig.ROOT / "scripts/rust_host_stage.py")
        stage = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stage)
        self.assertEqual(stage.HOSTED_TOOLCHAIN, self.FIXTURE)
        self.assertEqual(rig.BREENIX_TOOLCHAIN, self.FIXTURE)

        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        work = Path(temp.name)
        standard = work / "ext2-aarch64.img"
        standard.write_bytes(b"\0" * 1024)
        calls = []

        def run_command(words, **kwargs):
            words = [str(word) for word in words]
            calls.append(words)
            if words[-2:-1] and words[1].endswith("create-breenfs-root.sh"):
                Path(words[-1]).write_bytes(b"breenfs")

        with mock.patch.object(rig, "STANDARD_IMAGE", standard), \
                mock.patch.object(rig, "BREENIX_IMAGE", work / "out/root.raw"), \
                mock.patch.object(rig, "DISK_BYTES", 4096), \
                mock.patch.object(rig, "run_command", run_command):
            rig.stage_image({})
        staging = [words for words in calls
                   if words[1:2] == [str(rig.ROOT / "scripts/rust_host_stage.py")]]
        self.assertEqual(len(staging), 1, calls)
        [words] = staging
        self.assertEqual(words[words.index("--arch") + 1], "aarch64")
        self.assertTrue({"rustc", "cargo"} <= set(words[words.index("--suite") + 1].split(",")))
        # It stages the copy that becomes the BreenFS root, before the conversion.
        image = words[words.index("--image") + 1]
        converted = [calls.index(other) for other in calls
                     if other[1].endswith("create-breenfs-root.sh") and other[2] == image]
        self.assertTrue(converted and converted[0] > calls.index(words), calls)
        self.assertEqual((work / "out/root.raw").read_bytes(), b"breenfs")


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
            bench.guests = {system: mock.Mock(profiles=["hvf-host"])}
            bench.build = {"commit": "0" * 40, "artifacts": {}}
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


class LaunchProfile(unittest.TestCase):
    """The series name the accelerator and CPU the launcher resolved, not the one the
    rig expects: BREENIX_ARM64_QEMU_ACCEL=tcg gives TCG with -cpu max."""

    def test_a_tcg_launch_is_recorded_as_tcg(self):
        argv = [str(word) for word in rig.Guest("linux", Path("/rig"), Path("/disk"),
                                                headed=False).argv()]
        with mock.patch.dict(rig.os.environ, {"BREENIX_ARM64_QEMU_ACCEL": "tcg"}):
            profile = rig.launch_profile(argv)
        self.assertEqual(profile, "tcg-max")
        with mock.patch.object(rig, "qemu_version", lambda: "11.0.3"), \
                mock.patch.object(rig, "host_description", lambda: "Mac"):
            for text in (rig.hardware(profile), rig.vm_shape(profile)):
                self.assertIn("TCG -cpu max", text)
                self.assertNotIn("HVF", text)


class RestartableGuest:
    """A guest that boots when started; its QEMU stays up until it is stopped."""

    def __init__(self):
        self.boots = 1
        self.running = False
        self.home = "/etc/breenix/test-ssh/home"

    def alive(self):
        return self.running

    def start(self):
        self.boots += 1
        self.running = True

    def stop(self):
        self.running = False


class RestartThatFailsSetup(unittest.TestCase):
    """A guest whose restart fails setup (a wrong root, a failed upload) takes no
    samples, and the sidecar keeps no root line for that boot."""

    def test_the_guest_is_not_sampled_after_setup_fails(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        out = Path(temp.name)
        bench = rig.Rig.__new__(rig.Rig)
        bench.args = type("Args", (), {"run": "rig"})()
        bench.run_dir = out
        bench.out = {"breenix": out}
        bench.notes = {"breenix": []}
        bench.roots = {"breenix": {1: "none / breenfs rw 0 0"}}
        guest = RestartableGuest()
        bench.guests = {"breenix": guest}

        def setup(system):
            raise RuntimeError("breenix: the root is none / ext2 rw 0 0, not breenfs read-write")

        bench.setup = setup
        bench.endpoint = type("Endpoint", (), {"server_address": ("127.0.0.1", 9)})()
        bench.bbench = mock.Mock(side_effect=AssertionError("sampled a guest that failed setup"))
        with mock.patch("sys.stdout"):
            bench.sample("getpid", 2, "breenix")
        self.assertFalse(guest.alive())
        [receipt] = rig.collect_receipts(out, ["rig"], "scp-upload")["getpid"]
        self.assertEqual(receipt["error"], "breenix VM could not be restarted")
        self.assertEqual(bench.roots["breenix"], {1: "none / breenfs rw 0 0"})


class GuestWhoseWarmupFails(RecordingGuest):
    """Cargo fails the no-op warm-up; every other command succeeds."""

    def ssh(self, words, **kwargs):
        words = list(map(str, words))
        if not any(word.startswith("qemu-vol-") for word in words):
            return rig.subprocess.CompletedProcess(words, 0, b"", b"")
        result = super().ssh(words, **kwargs)
        receipt = json.loads(result.stdout)
        if receipt["sample"].endswith("noop-seed"):
            receipt["error"] = "cargo build exited 101"
        return rig.subprocess.CompletedProcess(words, 0, json.dumps(receipt).encode(), b"")


class CargoNoopWarmup(unittest.TestCase):
    """The no-op row times a build of an already built target. When the warm-up build
    fails, the next build would do the real work, so it is not timed as a no-op."""

    def test_a_failed_warmup_stops_the_timed_build(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        out = Path(temp.name)
        bench = rig.Rig.__new__(rig.Rig)
        bench.args = type("Args", (), {"run": "qemu-vol"})()
        bench.out = {"breenix": out}
        guest = GuestWhoseWarmupFails("/etc/breenix/test-ssh/home")
        bench.guests = {"breenix": guest}
        bench.endpoint = type("Endpoint", (), {"server_address": ("127.0.0.1", 9)})()
        bench.restart = lambda system: None
        for number in (1, 2):
            bench.sample("cargo-noop", number, "breenix")
        timed = [argv for argv in guest.argvs
                 if any(word.startswith("qemu-vol-cargo-noop-") for word in argv)]
        self.assertEqual(timed, [])
        groups = rig.collect_receipts(out, ["qemu-vol"], "scp-upload")
        errors = [sample["error"] for sample in groups["cargo-noop"]]
        self.assertEqual(len(errors), 2)
        for error in errors:
            self.assertIn("warm-up", error or "")
            self.assertIn("cargo build exited 101", error or "")


class RunIds(unittest.TestCase):
    """A run ID the series writer would refuse is refused before anything boots."""

    def refused(self, run):
        reached = mock.Mock(side_effect=AssertionError(f"{run!r} reached the run"))
        with mock.patch.object(rig, "git", lambda *args: ""), \
                mock.patch.object(rig, "Rig", reached), \
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

    def test_the_run_keeps_the_build_it_verified(self):
        """A subsequent build-breenix can replace the worktree's stamp and artifacts
        while a run samples. The run boots, uploads and records the build it verified."""
        rig.write_build_stamp(self.stamp, "c0ffee", self.artifacts)
        record, inputs = rig.snapshot_inputs(self.stamp, "c0ffee", self.artifacts,
                                             self.dir / "run/inputs")
        verified = {str(path): path.read_bytes() for path in self.artifacts}
        for path in self.artifacts:
            path.write_bytes(b"another build")
        rig.write_build_stamp(self.stamp, "0ther", self.artifacts)
        for path in self.artifacts:
            self.assertEqual(inputs[path].read_bytes(), verified[str(path)], path)
            self.assertEqual(rig.sha256(inputs[path]), record["artifacts"][str(path)], path)
        guest = rig.Guest("breenix", self.dir, self.dir / "disk", headed=False,
                          kernel=inputs[self.artifacts[0]])
        self.assertIn(str(inputs[self.artifacts[0]]), [str(word) for word in guest.argv()])

        bench = rig.Rig.__new__(rig.Rig)
        bench.args = type("Args", (), {"run": "qemu-vol", "pr": None})()
        out = self.dir / "out"
        out.mkdir()
        bench.out = {"breenix": out}
        bench.guests = {"breenix": mock.Mock(profiles=["hvf-host"])}
        bench.notes = {"breenix": []}
        bench.build = record
        bench.started = rig.datetime(2026, 11, 27, tzinfo=rig.timezone.utc)
        with mock.patch.object(rig, "SERIES_DIR", self.dir / "series"), \
                mock.patch.object(rig, "BUILD_STAMP", self.stamp), \
                mock.patch.object(rig, "qemu_version", lambda: "11.0.3"), \
                mock.patch.object(rig, "host_description", lambda: "Mac"), \
                mock.patch("sys.stdout"):
            path = bench.write("breenix", "c0ffee" + "0" * 34, {}, {}, {})
        sidecar = json.loads((Path(path).parent / "qemu-vol-breenix.provenance.json").read_text())
        self.assertEqual(sidecar["provenance"]["toolchain"]["stamp"], record)

    def test_a_stamp_missing_an_artifact_is_refused(self):
        rig.write_build_stamp(self.stamp, "c0ffee", self.artifacts[:1])
        with self.assertRaisesRegex(ValueError, "bbench"):
            rig.verify_build_stamp(self.stamp, "c0ffee", self.artifacts)


class RunStorage(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name)
        self.cache = self.home / "Library/Caches/breenix-bench"
        self.series = self.home / "series"

    def worktree(self, name):
        root = self.home / name
        bench = root / "userland/bbench"
        bench.mkdir(parents=True)
        for relative in ("docs", "toolchain"):
            (root / relative).symlink_to(rig.ROOT / relative)
        shutil.copyfile(rig.BENCH / "linux-arm64.json", bench / "linux-arm64.json")
        script = bench / "rig.py"
        shutil.copyfile(rig.__file__, script)
        spec = importlib.util.spec_from_file_location(name, script)
        lane = importlib.util.module_from_spec(spec)
        with mock.patch.object(Path, "home", return_value=self.home):
            spec.loader.exec_module(lane)
        return lane

    def test_overlapping_worktrees_publish_their_own_image_and_stamp(self):
        lanes = [self.worktree(name) for name in ("lane-a", "lane-b")]
        building = threading.Barrier(2)
        first_published = threading.Event()
        key_generations = []

        def build(lane, index):
            commit = str(index) * 40
            payload = commit.encode()
            libc = lane.ROOT / "libc-input"
            libc.write_bytes(b"libc")
            kd = lane.ROOT / "kd"
            kd.mkdir()
            env = {"BREENIX_KD_LIBC_INPUT": str(libc), "BREENIX_KD_BUILD_DIR": str(kd)}
            lane.git = lambda *words: commit if words[0] == "rev-parse" else ""
            lane.toolchain_build_env = lambda: env
            lane.BREENIX_BUILD = (["fixture-build"],)
            lane.DISK_BYTES = 4096

            def command(words, **kwargs):
                if words[0] == "fixture-build":
                    building.wait(timeout=10)
                    if index == 2 and not first_published.wait(timeout=10):
                        raise TimeoutError("the first lane did not publish")
                    for path in (*lane.BREENIX_ARTIFACTS, lane.STANDARD_IMAGE):
                        if path != lane.BREENIX_IMAGE:
                            path.parent.mkdir(parents=True, exist_ok=True)
                            path.write_bytes(payload)
                elif Path(words[1]).name == "create-breenfs-root.sh":
                    Path(words[-1]).write_bytes(Path(words[-2]).read_bytes())
                elif words[0] == "ssh-keygen":
                    key_generations.append(index)
                    return rig.run_command(words, **kwargs)

            lane.run_command = command
            lane.build_breenix()
            if index == 1:
                first_published.set()

        with ThreadPoolExecutor(max_workers=2) as pool:
            builds = [pool.submit(build, lane, index) for index, lane in enumerate(lanes, 1)]
            for result in builds:
                result.result()
        self.assertNotEqual(lanes[0].BREENIX_IMAGE, lanes[1].BREENIX_IMAGE)
        self.assertNotEqual(lanes[0].BUILD_STAMP, lanes[1].BUILD_STAMP)
        self.assertEqual(lanes[0].BREENIX_KEY, lanes[1].BREENIX_KEY)
        self.assertEqual(len(key_generations), 1)
        for index, lane in enumerate(lanes, 1):
            commit = str(index) * 40
            self.assertEqual(lane.BREENIX_IMAGE.read_bytes()[:40], commit.encode())
            self.assertEqual(lane.verify_build_stamp(lane.BUILD_STAMP, commit,
                                                     lane.BREENIX_ARTIFACTS), commit)

    def run_evidence(self, run_dir):
        files = {
            "breenix-disk.raw": b"breenix disk", "linux-disk.raw": b"linux disk",
            "breenix/sample.json": b'{"sample":"sample"}', "serial-breenix-1.txt": b"serial",
            "qemu-breenix-1.argv": b"argv", "qemu-linux-1.log": b"log",
            "inputs/1-root.raw": b"legacy snapshot",
        }
        for name, data in files.items():
            path = run_dir / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        return files

    def assert_only_disks_removed(self, run_dir, files):
        kept = {name: data for name, data in files.items()
                if name not in ("breenix-disk.raw", "linux-disk.raw")}
        self.assertEqual({str(path.relative_to(run_dir)): path.read_bytes()
                          for path in run_dir.rglob("*") if path.is_file()}, kept)

    def test_finished_run_discards_only_its_disks_after_both_series(self):
        bench = rig.Rig.__new__(rig.Rig)
        bench.run_dir = self.cache / "runs" / "finished"
        files = self.run_evidence(bench.run_dir)
        bench.args = SimpleNamespace(cases="getpid", count=1)
        bench.boot = mock.Mock(return_value="commit")
        bench.tcp_counters = bench.sample = bench.facts = mock.Mock()
        bench.guests = {system: mock.Mock() for system in rig.SYSTEMS}
        self.series.mkdir()

        def write(system, *_):
            for name in ("breenix-disk.raw", "linux-disk.raw"):
                self.assertTrue((bench.run_dir / name).is_file())
            for guest in bench.guests.values():
                guest.stop.assert_called_once()
            path = self.series / f"finished-{system}.series.json"
            path.write_text("{}")
            return path

        bench.write = write
        self.assertEqual(bench.run(), [self.series / f"finished-{system}.series.json"
                                      for system in rig.SYSTEMS])
        self.assert_only_disks_removed(bench.run_dir, files)

    def test_prune_removes_only_disks_from_old_runs_with_both_series(self):
        self.series.mkdir()
        runs = self.cache / "runs"
        for name in ("old", "recent", "incomplete"):
            files = self.run_evidence(runs / name)
            systems = rig.SYSTEMS[:1] if name == "incomplete" else rig.SYSTEMS
            for system in systems:
                (self.series / f"{name}-{system}.series.json").write_text("{}")
            if name != "recent":
                past = time.time() - 7200
                os.utime(runs / name, (past, past))

        with mock.patch.object(rig, "CACHE", self.cache), \
                mock.patch.object(rig, "SERIES_DIR", self.series):
            self.assertEqual(rig.cli(["prune"]), 0)
        self.assert_only_disks_removed(runs / "old", files)
        for name in ("recent", "incomplete"):
            for filename, data in files.items():
                self.assertEqual((runs / name / filename).read_bytes(), data)
        self.assertEqual(len(list(self.series.glob("*.series.json"))), 5)


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
