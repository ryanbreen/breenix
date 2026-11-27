"""A rig series stays inside the limits `vigil-cli breenix bench record` enforces."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import rig
import series
import vigil_rules

LONG_UNAME = ("Linux 6.8.0-147-generic #147-Ubuntu SMP PREEMPT_DYNAMIC Thu Sep 24 11:39:52 UTC "
              "2026 aarch64")
LONG_HOST = "Apple M5 Max (Mac17,6), macOS 27.0.1 " + "with a long description " * 8
# An SSH failure names the forwarded loopback address, which Vigil refuses as an IPv4 literal.
SSH_ERROR = ("no receipt (ssh exit 255: ssh: connect to host 127.0.0.1 port 50123: "
             "Connection refused " + "x" * 280 + ")")


class Args:
    run = "qemu-20261127-limits"
    pr = 3999


class LongProvenanceRun(unittest.TestCase):
    """A run with a long uname, host and counters, restart notes and failed samples."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.dir = Path(temp.name)
        self.series_dir = self.dir / "series"
        for patch in (mock.patch.object(rig, "SERIES_DIR", self.series_dir),
                      mock.patch.object(rig, "qemu_version", lambda: "11.0.3 (long build tag)"),
                      mock.patch.object(rig, "host_description", lambda: LONG_HOST)):
            patch.start()
            self.addCleanup(patch.stop)

    def write(self, system, facts):
        bench = rig.Rig.__new__(rig.Rig)
        bench.args = Args()
        out = self.dir / system
        out.mkdir()
        bench.out = {system: out}
        bench.notes = {system: [f"VM restarted after boot {n}" for n in (1, 2)]}
        bench.started = rig.datetime(2026, 11, 27, 22, 0, tzinfo=rig.timezone.utc)
        samples = (("getpid", 1, None), ("getpid", 2, SSH_ERROR),
                   ("malloc-1t", 1, SSH_ERROR), ("malloc-1t", 2, SSH_ERROR.replace("50123", "50124")))
        for case, number, error in samples:
            sample = f"{Args.run}-{case}-{number}"
            receipt = {"sample": sample, "argv": [case], "wall_ns": 10**9,
                       "metrics": {} if error else {"getpid_ns": 2 * 10**8}, "error": error,
                       "before": {}, "after": {}}
            (out / f"{sample}.json").write_text(json.dumps(receipt))
        counters = {f"counter_{n}": n * 1000 for n in range(40)}
        path = bench.write(system, "731f1d00" + "0" * 32, counters, counters, facts)
        return Path(path)

    def check(self, path):
        series = json.loads(path.read_text())
        self.assertEqual(vigil_rules.problems(series), [])
        sidecar = path.parent / f"{series['id']}.provenance.json"
        self.assertTrue(sidecar.exists(), sidecar)
        self.assertIn(sidecar.name, series["note"])
        return series, json.loads(sidecar.read_text())

    def test_the_linux_series_fits_and_its_sidecar_keeps_the_full_provenance(self):
        series, sidecar = self.check(self.write("linux", {"kernel": LONG_UNAME,
                                                          "toolchain": "rustc 1.90.0-nightly"}))
        text = json.dumps(sidecar)
        for detail in (LONG_UNAME, rig.PIN["url"], rig.PIN["sha256"], LONG_HOST, "counter_39"):
            self.assertIn(detail, text)
        self.assertTrue(series["kernelCommit"].startswith("Linux 6.8.0-147-generic"))

    def test_the_breenix_series_fits_and_keeps_each_failure_in_the_sidecar(self):
        series, sidecar = self.check(self.write("breenix", {}))
        rows = {row["id"]: row for row in series["rows"]}
        self.assertEqual((rows["process.getpid"]["completed"], rows["process.getpid"]["attempts"]),
                         (1, 2))
        self.assertEqual(rows["memory.malloc-1t"]["state"], "failed")
        # The row's reason is shortened to fit; the sidecar keeps both errors in full.
        self.assertIn(SSH_ERROR.replace("50123", "50124"), json.dumps(sidecar))


class CredentialRedaction(unittest.TestCase):
    """Text that looks like a credential is removed whole, never just its first
    character or opening marker, from the series and from its sidecar."""

    # Dummy values: none of these is a real credential.
    TOKEN = "dummyTOKENvalue0123456789abcdef"
    WORDS = "two dummy words"
    KEY_BODY = ("b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAAB",
                "AAAAMwAAAAtzc2gtZWQyNTUxOQAAACDummyKeyBodyLineTwo")

    def test_no_part_of_a_credential_reaches_either_file(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        directory = Path(temp.name)
        key = ("-----BEGIN OPENSSH PRIVATE KEY-----\n" + "\n".join(self.KEY_BODY)
               + "\n-----END OPENSSH PRIVATE KEY-----")
        note = (f"operator note: token={self.TOKEN} and password: \"{self.WORDS}\" "
                f"then {key} done")
        error = f"setup failed with api_key={self.TOKEN}"
        sample = "cred-getpid-1"
        groups = {"getpid": [{"sample": sample, "argv": ["getpid"], "wall_ns": 0, "metrics": {},
                              "error": error, "before": {}, "after": {}}]}
        metadata = {"system": "breenix", "target": "arm64-qemu", "hardware": "QEMU",
                    "kernelCommit": "0" * 8, "userlandCommit": "0" * 8, "toolchain": "rustc",
                    "session": "cred", "pr": None, "note": note}
        path = series.write_series(directory, "cred-breenix", "2026-11-27T22:00:00Z", metadata,
                                   groups, provenance={"note": note})
        written = json.loads(path.read_text())
        self.assertEqual(vigil_rules.problems(written), [])
        for name in (path.name, "cred-breenix.provenance.json"):
            text = (directory / name).read_text()
            for secret in (self.TOKEN, self.TOKEN[1:], self.WORDS, *self.KEY_BODY):
                self.assertNotIn(secret, text, name)
        # The text around the credentials survives.
        self.assertIn("operator note", written["note"])
        self.assertIn("done", written["note"])


if __name__ == "__main__":
    unittest.main()
