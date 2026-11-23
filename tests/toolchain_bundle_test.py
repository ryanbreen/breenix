"""Check the producer record and receipts of a built toolchain bundle."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
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
                         {"bcc", "bmake"})
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
                program = self.dev / "bin" / {"bcc": "cc", "bmake": "make"}.get(name, name)
                expected = {
                    "cargoPackage": name,
                    "rustForkCommit": fork,
                    "rustc": rustc,
                    "target": f"{arch}-breenix",
                    "sha256": digest(program),
                }
                if name in ("bcc", "bmake"):
                    expected["path"] = f"bin/{program.name}"
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
            "target": f"{self.record['arch']}-breenix",
            "sha256": digest(executable),
        })
        self.assertFalse(any(path.suffix == ".c" for path in (WORKSPACE / "bcc").rglob("*")))

    def make_built_by_rustc(self):
        entry = self.record["tools"]["bmake"]
        executable = self.dev / "bin/make"
        self.assertEqual(entry["path"], "bin/make")
        self.assertEqual(entry["sha256"], digest(executable))
        manifest = (self.dev / "manifest.sha256").read_text()
        self.assertIn(f"{digest(executable)}  opt/breenix-dev/25.2/bin/make\n", manifest)
        for category in ("make", "project"):
            for source in (WORKSPACE / "corpus" / category).iterdir():
                if category == "project" and source.name == "value.s" and self.record["arch"] == "aarch64":
                    source = WORKSPACE / "corpus/project-aarch64/value.s"
                with self.subTest(fixture=source.name):
                    path = f"usr/share/breenix/toolchain/{category}/{source.name}"
                    self.assertEqual(digest(self.bundle / "root" / path), digest(source))
                    self.assertIn(f"{digest(source)}  {path}\n", manifest)
        self.assertFalse(any(path.suffix == ".c" for path in (WORKSPACE / "bmake").rglob("*")))

        with tempfile.TemporaryDirectory(prefix="toolchain-stale-make-") as temp:
            bundle = Path(temp)
            tree = bundle / "root"
            shutil.copytree(self.bundle / "root", tree)
            for generated in ("manifest.sha256", "build.json"):
                (tree / "opt/breenix-dev/25.2" / generated).unlink()
            stale = "usr/share/breenix/toolchain/make/sample.src"
            (tree / stale).write_text("stale fixture\n")
            tool_record = bundle / "tools.json"
            tool_record.write_text(json.dumps(self.record["tools"]))
            sealed = subprocess.run([
                sys.executable, ROOT / "scripts/toolchain-image.py", "seal", bundle,
                self.record["commit"], self.record["arch"], "--tool-record", tool_record,
            ], capture_output=True, text=True, timeout=10)
            self.assertEqual(sealed.returncode, 1, sealed.stderr)
            self.assertIn(f"staged bytes differ from the source for {stale}", sealed.stderr)

    def arm_bundle_has_cc_and_make(self):
        self.assertEqual(set(self.record["tools"]), {"bas", "bld", "bar", "bnm", "bcc", "bmake"})
        self.assertEqual({path.name for path in (self.dev / "bin").iterdir()},
                         {"bas", "bld", "bar", "bnm", "cc", "make", "as", "ld"})
        for tool, alias in (("bas", "as"), ("bld", "ld")):
            self.assertEqual(digest(self.dev / "bin" / tool), digest(self.dev / "bin" / alias))
        self.assertEqual(self.record["candidateProducer"], "mac-bcc" if sys.platform == "darwin" else "host-bcc")

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

    def aarch64_stages_linker_objects(self):
        self.assertEqual(self.record["arch"], "aarch64")
        corpus = self.bundle / "root/usr/share/breenix/toolchain"
        manifest = (self.dev / "manifest.sha256").read_text().splitlines()
        for name in ("got-main", "got-entry", "archive-value-with-long-name", "tls-template"):
            path = corpus / "objs" / (name + ".o")
            self.assertTrue(path.is_file(), str(path))
            self.assertEqual(struct.unpack_from("<H", path.read_bytes(), 18)[0], 183)
            self.assertIn(f"{digest(path)}  usr/share/breenix/toolchain/objs/{name}.o", manifest)
        for name in ("got-main", "tls-template"):
            path = corpus / "linker" / (name + ".expect")
            source = WORKSPACE / "corpus/linker" / path.name
            self.assertEqual(path.read_bytes(), source.read_bytes())
            self.assertIn(f"{digest(path)}  usr/share/breenix/toolchain/linker/{path.name}", manifest)

    def aarch64_host_bld_links_staged_objects(self):
        self.assertEqual(self.record["arch"], "aarch64")
        corpus = self.bundle / "root/usr/share/breenix/toolchain"
        names = (WORKSPACE / "corpus/cases").read_text().splitlines()
        self.assertEqual(len(names), 30)
        with tempfile.TemporaryDirectory(prefix="toolchain-arm-host-link-") as temp:
            env = dict(os.environ, RUSTC=str(self.rustc))
            subprocess.run(["cargo", f"+{CHANNEL}", "build", "--release", "--locked",
                            "--manifest-path", WORKSPACE / "Cargo.toml", "--target-dir", temp,
                            "-p", "bld", "-p", "elfcheck"], env=env, check=True)
            bld, elfcheck = (Path(temp) / "release" / name for name in ("bld", "elfcheck"))
            obj = corpus / "objs"
            groups = [(name, [obj / (name + ".o")]) for name in names]
            groups += [("got-main", [obj / (name + ".o") for name in
                                      ("got-main", "got-entry", "archive-value-with-long-name")]),
                       ("tls-template", [obj / "tls-template.o"])]
            for name, inputs in groups:
                with self.subTest(program=name):
                    output = Path(temp) / (name + ".elf")
                    subprocess.run([bld, "-T", self.dev / "linker.ld", "--gc-sections", "-o", output,
                                    *inputs, self.dev / "lib/libc.a"], check=True, timeout=60)
                    subprocess.run([elfcheck, "--machine", "aarch64", "--exec", output],
                                   check=True, timeout=10)

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
    checks.extend(["ToolchainBundleTest.cc_built_by_rustc",
                   "ToolchainBundleTest.make_built_by_rustc"])
    if json.loads((args.bundle / "build.json").read_text())["arch"] == "aarch64":
        checks.extend(["ToolchainBundleTest.arm_bundle_has_cc_and_make",
                       "ToolchainBundleTest.arm_encoding_inputs_are_staged",
                       "ToolchainBundleTest.aarch64_stages_linker_objects",
                       "ToolchainBundleTest.aarch64_host_bld_links_staged_objects"])
    if args.baseline is not None:
        checks.append("ToolchainBundleTest.x86_artifacts_equal_baseline")
    if args.probe is not None:
        checks.append("ToolchainBundleTest.read_fault_probe_is_not_shipped")
    unittest.main(argv=[sys.argv[0], *names], defaultTest=checks)
