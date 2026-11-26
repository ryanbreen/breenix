"""Verify the K6 link record against the retained Mac capture."""

import json
import os
from pathlib import Path, PurePosixPath
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECORD = ROOT / "toolchain/rust-host/k6/x86_64.json"
FIXTURE = ROOT / "build/rustc-port/k6/fixture"


class K6LinkRecord(unittest.TestCase):
    def test_k6_link_lines_recorded(self):
        self.assertTrue(RECORD.is_file(), "K6 has no recorded twin linker invocations")
        sys.path.insert(0, str(ROOT / "scripts/rust-host"))
        import k6_record as k6

        fixture = Path(os.environ.get("BREENIX_K6_FIXTURE", FIXTURE)).resolve()
        record = json.loads(RECORD.read_text())
        k5 = json.loads((ROOT / "toolchain/rust-host/k5/x86_64.json").read_text())
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
        bld = k6.bld_relocations()
        for name, line in lines.items():
            with self.subTest(link=name):
                argv = line["argv"]
                self.assertEqual(line["argument_kinds"], [arg.kind for arg in k6.classify(argv)])
                sources = k6.link_inputs(argv, fixture)
                self.assertEqual(line["inputs"], sorted(sources))
                used.update(sources)
                hosted = name == "catch-unwind"
                target = "x86_64-breenix-hosted" if hosted else "x86_64-breenix"
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
                self.assertEqual(k6.reference_headers(fixture / reference, k6.llvm_tools()),
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
                self.assertEqual(entry, k6.inspect_input(fixture / path, tools))
                if relative.suffix == ".rlib":
                    self.assertIn("lib.rmeta", entry["metadata_members"])


if __name__ == "__main__":
    unittest.main()
