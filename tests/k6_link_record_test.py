"""Verify the K6 link record against the retained Mac capture."""

import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECORD = ROOT / "toolchain/rust-host/k6/x86_64.json"
FIXTURE = ROOT / "build/rustc-port/k6/fixture"


class K6LinkRecord(unittest.TestCase):
    def test_k6_link_lines_recorded(self):
        self.check_k6_link_lines("x86_64")

    def test_k6_link_lines_recorded_aarch64(self):
        self.check_k6_link_lines("aarch64")

    def test_aarch64_retained_fixture_checks_negative_archive(self):
        key = "BREENIX_K6_AARCH64_FIXTURE"
        default = Path.home() / "Library/Caches/breenix-e25/k6/aarch64"
        fixture = Path(os.environ.get(key, default)).expanduser().resolve()
        checker = [sys.executable, ROOT / "scripts/rust-host/check-k6-fixture.py", "--arch", "aarch64"]
        with tempfile.TemporaryDirectory() as directory:
            projected = Path(directory)
            for source in fixture.iterdir():
                if source.name != "control":
                    (projected / source.name).symlink_to(source)
            archive = projected / "control/non-elf.rlib"
            archive.parent.mkdir()
            archive.write_bytes(b"wrong archive")
            env = dict(os.environ, **{key: str(projected)})
            result = subprocess.run(checker, env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("control/non-elf.rlib: SHA-256 differs", result.stderr)

            archive.unlink()
            archive.symlink_to(fixture / archive.relative_to(projected))
            result = subprocess.run(checker, env=env, capture_output=True, text=True, check=True)
            self.assertIn("verified 89 SHA-256 digests and 8 argv records", result.stdout)

    def check_k6_link_lines(self, arch):
        record_path = ROOT / f"toolchain/rust-host/k6/{arch}.json"
        self.assertTrue(record_path.is_file(), "K6 has no recorded twin linker invocations")
        sys.path.insert(0, str(ROOT / "scripts/rust-host"))
        import k6_record as k6

        fixture = Path(os.environ.get("BREENIX_K6_FIXTURE", FIXTURE) if arch == "x86_64" else os.environ.get("BREENIX_K6_AARCH64_FIXTURE", ROOT / "build/rustc-port/k6/aarch64")).resolve()
        record = json.loads(record_path.read_text())
        k5 = json.loads((ROOT / f"toolchain/rust-host/k5/{arch}.json").read_text())
        self.assertEqual(record["rustc_sha256"], k5["mac_rustc_sha256"])
        self.assertEqual(record["rustc_commit"], k5["fork_commit"])
        self.assertEqual(record["cargo_channel"], "nightly-2025-06-24")
        self.assertEqual(set(record["cargo_environment"]), {
            "RUST_TARGET_PATH", "RUSTC", "CARGO_HOME", "CARGO_ENCODED_RUSTFLAGS",
        })
        self.assertEqual(record["rlib_metadata_member"], "lib.rmeta")
        inputs = record["inputs"]
        lines = record["links"]
        self.assertEqual(set(lines), {
            "bas", "bld", "bar", "bnm", "bcc", "bmake", "panic-abort", "catch-unwind",
        })
        used = set()
        observed = set()
        bld = k6.bld_relocations() if arch == "x86_64" else k6.bld_relocations(arch)
        for name, line in lines.items():
            with self.subTest(link=name):
                argv = line["argv"]
                self.assertEqual(line["argument_kinds"], [arg.kind for arg in k6.classify(argv)])
                sources = k6.link_inputs(argv, fixture)
                self.assertEqual(line["inputs"], sorted(sources))
                used.update(sources)
                hosted = name == "catch-unwind"
                target = f"{arch}-breenix-hosted" if hosted else f"{arch}-breenix"
                lib = "lib/hosted" if hosted else "lib/tools"
                script = "linker-hosted.ld" if hosted else "linker.ld"
                self.assertIn(f"{lib}/{script}", sources)
                self.assertIn(f"{lib}/libc.a", sources)
                self.assertEqual(f"{lib}/libbunwind.a" in sources, hosted)
                if hosted:
                    self.assertIn("--eh-frame-hdr", argv)
                std, = [path for path in sources if Path(path).name.startswith("libstd-")]
                self.assertIn(f"sysroot/lib/rustlib/{target}/lib/", std)
                self.assertEqual(inputs[std]["sha256"], k5["sysroot"]["files_sha256"][
                    std.removeprefix("sysroot/")])
                kinds = {kind for path in sources for kind in inputs[path]["relocations"]}
                observed.update(kinds)
                self.assertEqual(line["relocations"], sorted(kinds))
                self.assertEqual(line["needs_bld"], sorted(kinds - bld))
                reference = f"links/{name}/reference.elf"
                self.assertEqual(argv[argv.index("-o") + 1], reference)
                self.assertEqual(k6.sha256(fixture / reference), line["reference_sha256"])
                self.assertEqual(k6.reference_headers(fixture / reference, k6.llvm_tools()) if arch == "x86_64" else k6.reference_headers(fixture / reference, k6.llvm_tools(), arch),
                                 hosted)

        self.assertEqual(set(inputs), used)
        self.assertEqual(record["relocations"], {
            "observed": sorted(observed),
            "supported_by_bld": sorted(observed & bld),
            "needs_bld": sorted(observed - bld),
        })
        tools = k6.llvm_tools()
        for path, entry in inputs.items():
            relative = PurePosixPath(path)
            self.assertFalse(relative.is_absolute() or ".." in relative.parts, path)
            with self.subTest(input=path):
                self.assertEqual(entry, k6.inspect_input(fixture / path, tools) if arch == "x86_64" else k6.inspect_input(fixture / path, tools, arch))
                if relative.suffix == ".rlib":
                    self.assertIn("lib.rmeta", entry["metadata_members"])


if __name__ == "__main__":
    unittest.main()
