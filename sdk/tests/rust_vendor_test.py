"""SDK directory-source packaging, including recursive in-tree dependencies."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("rust_vendor", ROOT / "sdk/builder/rust-vendor.py")
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


class RustVendorTests(unittest.TestCase):
    def test_real_sdk_crates_are_self_contained_and_checksummed(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            version = vendor.vendor_crate(ROOT / "libs/libbreenix", out, root=True)
            root = out / f"libbreenix-{version}"
            manifest = (root / "Cargo.toml").read_text()
            self.assertNotIn("path =", manifest)
            self.assertNotIn("[workspace]", manifest)
            self.assertNotIn("libgfx", manifest)
            self.assertNotIn("suite", manifest)
            self.assertFalse((root / "src/suite.rs").exists())
            self.assertNotIn('pub mod suite;', (root / "src/lib.rs").read_text())
            self.assertIn('gpu-abi = { version = "=0.1.0" }', manifest)
            self.assertTrue((out / "gpu-abi-0.1.0/src/lib.rs").exists())
            for crate in out.iterdir():
                checksums = json.loads((crate / ".cargo-checksum.json").read_text())
                self.assertIsNone(checksums["package"])
                actual = {p.relative_to(crate).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in crate.rglob("*") if p.is_file() and p.name != ".cargo-checksum.json"}
                self.assertEqual(checksums["files"], actual)

    def test_recursive_dependency_versions_and_features_survive(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for name, version, dependency in (("a", "1.0.0", 'b = { path = "../b", features = ["extra"] }'),
                                               ("b", "2.0.0", 'c = { path = "../c" }'),
                                               ("c", "3.0.0", "")):
                src = base / name
                (src / "src").mkdir(parents=True)
                (src / "src/lib.rs").write_text("pub const VALUE: u32 = 1;\n")
                (src / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "{version}"\n[dependencies]\n{dependency}\n[features]\nextra = []\n')
            out = base / "out"
            vendor.vendor_crate(base / "a", out)
            self.assertEqual({p.name for p in out.iterdir()}, {"a-1.0.0", "b-2.0.0", "c-3.0.0"})
            self.assertIn('b = { version = "=2.0.0", features = ["extra"] }', (out / "a-1.0.0/Cargo.toml").read_text())
            self.assertIn('c = { version = "=3.0.0" }', (out / "b-2.0.0/Cargo.toml").read_text())

    def test_registry_dependency_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "crate"
            (src / "src").mkdir(parents=True)
            (src / "src/lib.rs").write_text("")
            (src / "Cargo.toml").write_text('[package]\nname = "bad"\nversion = "1.0.0"\n[dependencies]\nforeign = "1"\n')
            with self.assertRaisesRegex(SystemExit, "not an in-tree path crate"):
                vendor.vendor_crate(src, Path(tmp) / "out")


if __name__ == "__main__":
    unittest.main()
