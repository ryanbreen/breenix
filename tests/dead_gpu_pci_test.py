"""Check that the legacy PCI GPU is gone and the GPU core still probes PCI."""

from pathlib import Path
import re
import unittest


KERNEL = Path(__file__).resolve().parents[1] / "kernel" / "src"


class DeadGpuPciTest(unittest.TestCase):
    def test_legacy_driver_is_absent(self):
        virtio = KERNEL / "drivers" / "virtio"
        for module in ("gpu_pci", "virgl"):
            with self.subTest(module=module):
                self.assertFalse((virtio / f"{module}.rs").exists())
                self.assertNotRegex(
                    (virtio / "mod.rs").read_text(), rf"\bmod\s+{module}\s*;"
                )
        references = [
            str(source.relative_to(KERNEL))
            for source in sorted(KERNEL.rglob("*.rs"))
            if source != virtio / "gpu_pci.rs" and "gpu_pci" in source.read_text()
        ]
        self.assertEqual(references, [], "kernel files still refer to the legacy module")

    def test_gpu_core_still_probes_arm64_pci(self):
        drivers = (KERNEL / "drivers" / "mod.rs").read_text()
        for platform, marker in (
            ("QEMU", "PCI ECAM at {:#x}, enumerating PCI bus..."),
            ("Parallels", "PCI-based platform (Parallels): enumerate PCI bus"),
        ):
            with self.subTest(platform=platform):
                after_marker = drivers.split(marker, 1)[1]
                self.assertRegex(
                    after_marker,
                    re.compile(r"\A(?:(?!\breturn\b).)*crate::gpu::init\(\);", re.S),
                )


if __name__ == "__main__":
    unittest.main()
