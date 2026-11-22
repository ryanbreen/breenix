"""Compare suite-selected ext2 work copies of the same ordinary-layout seed."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/metal"))
from metal_root import require_e2fsprogs


class ToolchainImagePrefixTest(unittest.TestCase):
    def only_declared_paths_differ(self):
        mkfs, debugfs = require_e2fsprogs("mke2fs", "debugfs")
        with tempfile.TemporaryDirectory(prefix="toolchain-image-") as temporary:
            scratch = Path(temporary)
            base = scratch / "base"
            binaries = scratch / "binaries"
            base.mkdir()
            binaries.mkdir()
            bundle = scratch / "bundle"
            # This is a packaging fixture. The corpus build independently
            # verifies and inventories real reference ELFs.
            fixtures = {
                "opt/breenix-dev/25.2/include/stddef.h": b"support header\n",
                "opt/breenix-dev/25.2/bin/bas": b"\x7fELF assembler",
                "opt/breenix-dev/25.2/bin/bld": b"\x7fELF linker",
                "opt/breenix-dev/25.2/bin/bar": b"\x7fELF archiver",
                "opt/breenix-dev/25.2/bin/bnm": b"\x7fELF symbol reader",
                "usr/share/breenix/toolchain/assembler/sse-exit.link.json": b"link receipt\n",
                "usr/share/breenix/toolchain/objs/return.o": b"\x7fELF object",
                "usr/share/breenix/toolchain/return.elf": b"\x7fELF reference",
                "usr/share/breenix/toolchain/return.expect": b"42\n",
                "sbin/suite-toolchain": b"\x7fELF suite",
            }
            manifest = "".join(
                f"{hashlib.sha256(data).hexdigest()}  {path}\n"
                for path, data in sorted(fixtures.items())
            ).encode()
            record = json.dumps({
                "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "arch": "x86_64",
                "manifestSha256": hashlib.sha256(manifest).hexdigest(),
            }).encode() + b"\n"
            fixtures["opt/breenix-dev/25.2/manifest.sha256"] = manifest
            fixtures["opt/breenix-dev/25.2/build.json"] = record
            bundle.mkdir()
            (bundle / "build.json").write_bytes(record)
            for path, data in fixtures.items():
                target = bundle / "root" / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                target.chmod(0o755 if path.endswith(".elf") or "/bin/" in path
                             or path.startswith("sbin/") else 0o644)
            (binaries / "suite-toolchain.elf").write_bytes(fixtures["sbin/suite-toolchain"])
            (binaries / "suite-files-io.elf").write_bytes(b"\x7fELF unrelated suite")
            subprocess.run(
                ["sh", str(ROOT / "scripts/install-layout.sh"), str(base), str(binaries),
                 str(ROOT / "libs/libbreenix/src/layout.tsv")],
                env=dict(os.environ, BREENIX_EXT2_OWNERS=str(scratch / "owners")), check=True,
            )
            # Ordinary images get this shared parent from install-ui-references.
            (base / "usr/share/breenix").mkdir()
            seed = scratch / "ext2.img"
            subprocess.run([mkfs, "-q", "-t", "ext2", "-F", "-d", str(base), str(seed), "8192"],
                           check=True, capture_output=True)
            before = hashlib.sha256(seed.read_bytes()).digest()

            def stage(name, suites, use_bundle=False):
                output = scratch / f"{name}.img"
                command = ["bash", str(ROOT / "scripts/create_ext2_disk.sh"), "--stage-from", str(seed),
                           "--output", str(output), "--suite", suites]
                if use_bundle:
                    command += ["--toolchain-bundle", str(bundle)]
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return output

            def tree(image):
                destination = scratch / (image.stem + "-tree")
                destination.mkdir()
                subprocess.run([debugfs, "-R", f'rdump / "{destination}"', str(image)],
                               check=True, capture_output=True)
                result = {}
                for path in destination.rglob("*"):
                    name = str(path.relative_to(destination))
                    mode = path.lstat().st_mode
                    if path.is_symlink():
                        data = os.readlink(path)
                    elif stat.S_ISREG(mode):
                        data = path.read_bytes()
                    else:
                        data = None
                    result[name] = (mode, data)
                return result

            selected = tree(stage("selected", "files-io,toolchain", use_bundle=True))
            unselected = tree(stage("unselected", "files-io", use_bundle=True))
            ordinary = tree(seed)
            self.assertEqual(ordinary, unselected)
            self.assertEqual(ordinary, tree(stage("no-suite", "")))
            self.assertEqual(before, hashlib.sha256(seed.read_bytes()).digest())
            self.assertLess(ordinary.keys(), selected.keys())
            self.assertEqual(stat.S_IMODE(selected["opt/breenix-dev/25.2/bin/bas"][0]), 0o755)
            for tool in ("bld", "bar", "bnm"):
                self.assertEqual(stat.S_IMODE(selected[f"opt/breenix-dev/25.2/bin/{tool}"][0]), 0o755)
            self.assertEqual(stat.S_IMODE(selected["usr/share/breenix/toolchain/objs/return.o"][0]), 0o644)
            self.assertEqual(ordinary, {key: selected[key] for key in ordinary})
            added = selected.keys() - ordinary.keys()
            self.assertEqual({key: selected[key][1] for key in added if selected[key][1] is not None},
                             fixtures)
            prefixes = ("opt/breenix-dev", "usr/share/breenix/toolchain")
            for key in added - fixtures.keys():
                self.assertTrue(any(key == prefix or key.startswith(prefix + "/")
                                    for prefix in prefixes), key)

    test_only_declared_paths_differ = only_declared_paths_differ


if __name__ == "__main__":
    unittest.main(defaultTest="ToolchainImagePrefixTest.only_declared_paths_differ")
