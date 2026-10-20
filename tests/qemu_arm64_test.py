#!/usr/bin/env python3
"""Host checks for ARM64 accelerator selection; no VM is started."""

import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LAUNCHER = ROOT / "scripts/qemu-arm64.py"
command = runpy.run_path(str(LAUNCHER))["command"]
default_accelerator = runpy.run_path(str(LAUNCHER))["default_accelerator"]


class AcceleratorTests(unittest.TestCase):
    def test_default_hvf_is_scoped_to_apple_silicon(self):
        self.assertEqual(default_accelerator("Darwin", "arm64"), "hvf")
        for system, machine in (("Linux", "aarch64"), ("Linux", "x86_64"), ("Darwin", "x86_64")):
            with self.subTest(system=system, machine=machine):
                self.assertEqual(default_accelerator(system, machine), "tcg")

    def test_hvf_keeps_machine_properties_and_devices(self):
        result = command("qemu", ["-M", "virt,gic-version=2,highmem=off", "-cpu", "max",
                                  "-smp", "4", "-device", "virtio-blk-device,drive=ext2"],
                         "hvf", "Darwin", "arm64")
        self.assertEqual(result, ["qemu", "-accel", "hvf", "-M", "virt,highmem=off,gic-version=3",
                                  "-cpu", "host", "-smp", "4", "-device", "virtio-blk-device,drive=ext2"])

    def test_hvf_requires_a_representable_profile(self):
        for cpu in ("cortex-a72", "cortex-a57", "max,pauth=off"):
            with self.subTest(cpu=cpu), self.assertRaisesRegex(ValueError, "select TCG"):
                command("qemu", ["-cpu", cpu], "hvf", "Darwin", "arm64")

    def test_hvf_injects_host_cpu_and_accepts_machine_type(self):
        self.assertEqual(command("qemu", ["-machine", "type=virt"], "hvf", "Darwin", "arm64"),
                         ["qemu", "-accel", "hvf", "-machine", "type=virt,gic-version=3", "-cpu", "host"])

    def test_machine_accelerator_cannot_override_selection(self):
        for option in ("-M", "-machine"):
            with self.subTest(option=option), self.assertRaises(ValueError):
                command("qemu", [option, "virt,accel=tcg"], "hvf", "Darwin", "arm64")

    def test_tcg_preserves_cpu_and_gic_profiles(self):
        args = ["-machine", "virt,gic-version=3", "-cpu", "cortex-a72", "-smp", "4"]
        self.assertEqual(command("qemu", args, "tcg", "Darwin", "arm64"),
                         ["qemu", "-accel", "tcg", *args])

    def test_hvf_refuses_other_hosts(self):
        for system, machine in (("Linux", "aarch64"), ("Darwin", "x86_64")):
            with self.subTest(system=system, machine=machine), self.assertRaises(ValueError):
                command("qemu", ["-M", "virt", "-cpu", "max"], "hvf", system, machine)

    def test_invalid_options_fail(self):
        for accelerator, args in (("auto", []), ("tcg", ["-cpu"]),
                                  ("tcg", ["-accel", "hvf"]), ("hvf", ["-M", "vmapple"])):
            with self.subTest(accelerator=accelerator, args=args), self.assertRaises(ValueError):
                command("qemu", args, accelerator, "Darwin", "arm64")

    def test_default_named_cpu_runs_tcg_and_reports_effective_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "qemu"
            binary.write_text(f"#!{sys.executable}\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n")
            binary.chmod(0o755)
            env = {key: value for key, value in os.environ.items() if key != "BREENIX_ARM64_QEMU_ACCEL"}
            result = subprocess.run([sys.executable, str(LAUNCHER), "--launcher", "env", str(binary),
                                     "-cpu", "cortex-a72"], env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), ["-accel", "tcg", "-cpu", "cortex-a72"])
            self.assertIn("BREENIX_QEMU_CONFIG accel=tcg cpu=cortex-a72", result.stderr)

    def test_recorded_profile_matches_effective_tcg_cpu_without_launching(self):
        result = subprocess.run([sys.executable, str(LAUNCHER), "--print-profile", "/nonexistent/qemu",
                                 "-cpu", "cortex-a72"],
                                env={**os.environ, "BREENIX_ARM64_QEMU_ACCEL": "tcg"},
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "tcg-cortex-a72")

    def test_exec_preserves_arguments_and_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "fake qemu"
            binary.write_text(f"#!{sys.executable}\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\nsys.exit(37)\n")
            binary.chmod(0o755)
            result = subprocess.run([sys.executable, str(LAUNCHER), "--launcher", "env", str(binary), "-M", "virt",
                                     "-cpu", "max", "-drive", "file=a disk.img"],
                                    env=dict(os.environ, BREENIX_ARM64_QEMU_ACCEL="tcg"),
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 37)
            self.assertEqual(json.loads(result.stdout), ["-accel", "tcg", "-M", "virt", "-cpu", "max",
                                                         "-drive", "file=a disk.img"])


if __name__ == "__main__":
    unittest.main()
