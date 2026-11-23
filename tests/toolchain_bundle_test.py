"""Check the producer record and receipts of a built toolchain bundle."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = ROOT / "toolchain"
CHANNEL = tomllib.loads((WORKSPACE / "rust-toolchain.toml").read_text())["toolchain"]["channel"]


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


class ToolchainBundleTest(unittest.TestCase):
    def setUp(self):
        self.dev = self.bundle / "root/opt/breenix-dev/25.2"
        self.record = json.loads((self.bundle / "build.json").read_text())
        self.rustc = Path(subprocess.check_output(
            ["rustup", "which", "--toolchain", CHANNEL, "rustc"], text=True,
        ).strip())

    def tools_built_by_rustc(self):
        tools = self.record.get("tools")
        self.assertIsInstance(tools, dict, "bundle build record has no Cargo tool provenance")
        self.assertEqual(set(tools), {"bas", "bld", "bar", "bnm"} |
                         ({"bcc"} if self.record["arch"] == "x86_64" else set()))
        self.assertEqual(self.record, json.loads((self.dev / "build.json").read_text()))
        arch = self.record["arch"]
        machine = {"x86_64": 62, "aarch64": 183}[arch]  # ELF gABI, e_machine.
        fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
        rustc = {
            "version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
            "sha256": digest(self.rustc),
        }
        for name, entry in tools.items():
            with self.subTest(tool=name):
                program = self.dev / "bin" / ("cc" if name == "bcc" else name)
                expected = {
                    "cargoPackage": name,
                    "rustForkCommit": fork,
                    "rustc": rustc,
                    "target": f"{arch}-breenix",
                    "sha256": digest(program),
                }
                if name == "bcc":
                    expected["path"] = "bin/cc"
                self.assertEqual(entry, expected)
                with program.open("rb") as file:
                    header = file.read(20)
                self.assertEqual(header[:6], b"\x7fELF\x02\x01")
                self.assertEqual(struct.unpack_from("<HH", header, 16), (2, machine))

    def cc_built_by_rustc(self):
        entry = self.record["tools"]["bcc"]
        executable = self.dev / "bin/cc"
        fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
        self.assertEqual(entry, {
            "cargoPackage": "bcc",
            "path": "bin/cc",
            "rustForkCommit": fork,
            "rustc": {"version": subprocess.check_output([self.rustc, "--version"], text=True).strip(),
                      "sha256": digest(self.rustc)},
            "target": "x86_64-breenix",
            "sha256": digest(executable),
        })
        self.assertFalse(any(path.suffix == ".c" for path in (WORKSPACE / "bcc").rglob("*")))

    def arm_bundle_unchanged_until_a3(self):
        self.assertEqual(set(self.record["tools"]), {"bas", "bld", "bar", "bnm"})
        self.assertEqual({path.name for path in (self.dev / "bin").iterdir()},
                         {"bas", "bld", "bar", "bnm"})

    def arm_encoding_inputs_are_staged(self):
        relative = Path("usr/share/breenix/toolchain/assembler")
        manifest = (self.dev / "manifest.sha256").read_text().splitlines()
        for name in ("encodings.s", "encodings.hex"):
            with self.subTest(file=name):
                source = WORKSPACE / "corpus/assembler-aarch64" / name
                staged = self.bundle / "root" / relative / name
                self.assertTrue(staged.is_file(), f"ARM bundle is missing {relative / name}")
                self.assertEqual(staged.read_bytes(), source.read_bytes())
                self.assertIn(f"{digest(source)}  {relative / name}", manifest)

    def receipts_from_cargo_host_bas(self):
        assembler = self.bundle / "root/usr/share/breenix/toolchain/assembler"
        names = (("sse-exit", "x87-exit") if self.record["arch"] == "x86_64"
                 else ("fp-exit", "call-exit"))
        receipts = {name: json.loads((assembler / f"{name}.link.json").read_text()) for name in names}
        for name, receipt in receipts.items():
            self.assertIn("hostBas", receipt, f"{name} has no Cargo host bas provenance")

        # Rebuild the staged bas package for the host so its assembler can run
        # here. The guest object-runs case compares the same SHA-256 values
        # after assembling these sources with the staged guest ELF.
        version = subprocess.check_output([self.rustc, "-vV"], text=True)
        host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
        with tempfile.TemporaryDirectory(prefix="toolchain-host-bas-") as temp:
            env = dict(os.environ, RUSTC=str(self.rustc))
            for key in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
                env.pop(key, None)
            flags = []
            if env.get("BREENIX_USERSPACE_REMAP") == "1":
                cargo_home = Path(env.get("CARGO_HOME", Path.home() / ".cargo"))
                flags = [f"--remap-path-prefix={ROOT}=/breenix", f"--remap-path-prefix={cargo_home}=/cargo"]
            env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
            subprocess.run([
                "cargo", f"+{CHANNEL}", "build", "--release", "--locked", "-p", "bas",
                "--target", host, "--target-dir", temp, "--manifest-path", WORKSPACE / "Cargo.toml",
            ], cwd=WORKSPACE, env=env, check=True, timeout=60)
            bas = Path(temp) / host / "release/bas"
            producer = {"cargoPackage": "bas", "target": host, "sha256": digest(bas)}
            for name, receipt in receipts.items():
                with self.subTest(program=name):
                    self.assertEqual(receipt["hostBas"], producer)
                    obj = Path(temp) / f"{name}.o"
                    subprocess.run([bas, *(["--target", "aarch64"] if self.record["arch"] == "aarch64" else []),
                                    "-o", obj, assembler / f"{name}.s"], check=True, timeout=10)
                    self.assertEqual(receipt["inputObjects"], {obj.name: digest(obj)})
                    elf = assembler / f"{name}.elf"
                    self.assertEqual(receipt["elf"], {"name": elf.name, "sha256": digest(elf)})

    def x86_artifacts_equal_baseline(self):
        self.assertEqual(self.record["arch"], "x86_64")
        self.assertIsNotNone(self.baseline, "pass an origin/main x86 bundle with --baseline")
        self.assertEqual(json.loads((self.baseline / "build.json").read_text())["arch"], "x86_64")
        relative = "root/usr/share/breenix/toolchain/assembler"
        actual, expected = self.bundle / relative, self.baseline / relative
        self.assertEqual({p.name for p in actual.iterdir()}, {p.name for p in expected.iterdir()})
        for path in actual.iterdir():
            with self.subTest(artifact=path.name):
                if path.name.endswith(".link.json"):
                    receipt = json.loads(path.read_text())
                    baseline = json.loads((expected / path.name).read_text())
                    for field in ("inputObjects", "elf", "map"):
                        self.assertEqual(receipt[field], baseline[field])
                else:
                    self.assertEqual(path.read_bytes(), (expected / path.name).read_bytes())

    def read_fault_probe_is_not_shipped(self):
        self.assertIsNotNone(self.probe, "pass the guest-target bld-read-probe with --probe")
        shipped = self.dev / "bin/bld"
        probe = self.probe.read_bytes()
        with shipped.open("rb") as file:
            header = file.read(20)
        self.assertEqual(probe[:20], header, "the probe must be for the same ELF target")
        self.assertIn(b"read EINTR", probe)
        self.assertIn(b"read short", probe)
        self.assertNotEqual(digest(shipped), digest(self.probe))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--probe", type=Path)
    parser.add_argument("--baseline", type=Path)
    args, names = parser.parse_known_args()
    ToolchainBundleTest.bundle = args.bundle
    ToolchainBundleTest.probe = args.probe
    ToolchainBundleTest.baseline = args.baseline
    checks = ["ToolchainBundleTest.tools_built_by_rustc",
              "ToolchainBundleTest.receipts_from_cargo_host_bas"]
    if json.loads((args.bundle / "build.json").read_text())["arch"] == "x86_64":
        checks.append("ToolchainBundleTest.cc_built_by_rustc")
    else:
        checks.extend(["ToolchainBundleTest.arm_bundle_unchanged_until_a3",
                       "ToolchainBundleTest.arm_encoding_inputs_are_staged"])
    if args.baseline is not None:
        checks.append("ToolchainBundleTest.x86_artifacts_equal_baseline")
    if args.probe is not None:
        checks.append("ToolchainBundleTest.read_fault_probe_is_not_shipped")
    unittest.main(argv=[sys.argv[0], *names], defaultTest=checks)
