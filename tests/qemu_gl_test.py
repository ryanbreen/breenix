"""The GL QEMU's launcher option (scripts/boot-interactive.sh --gpu-gl) and its attestation check."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts/check-host-renderer.py"
METAL = "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)"


def record(event, renderer, vendor="Google Inc. (Apple)", version="OpenGL ES 3.0.0 (ANGLE 2.1)", ctx_id=1):
    return {"event": event, "ctx_id": ctx_id, "gl_vendor": vendor, "gl_renderer": renderer,
            "gl_version": version, "egl_vendor": "Google Inc. (Apple)", "egl_version": "1.5 (ANGLE 2.1)"}


def workload(renderer=METAL):
    """A context created and then submitted to, as a guest that renders leaves it."""
    return [record("context-create", renderer), record("first-submit-3d", renderer)]


class CheckHostRendererTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="hr-", dir="/tmp")
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def check(self, records=None, serial=None, fmt=1):
        path = self.dir / "host-renderer.json"
        if records is not None:
            path.write_text(json.dumps({"format": fmt, "pid": 1, "records": records}))
        args = [sys.executable, str(CHECK), str(path)]
        if serial is not None:
            (self.dir / "serial.txt").write_text(serial)
            args += ["--serial", str(self.dir / "serial.txt")]
        return subprocess.run(args, capture_output=True, text=True, timeout=10)

    def assertRefused(self, result, reason):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("REFUSED", result.stdout)
        self.assertIn(reason, result.stdout)

    def test_the_apple_gpu_through_metal_is_accepted(self):
        result = self.check(workload(), serial="VIRGL_CAPSET_RENDERER=Apple M1\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("accepted", result.stdout)
        self.assertIn(METAL, result.stdout)

    def test_a_missing_or_empty_attestation_is_refused(self):
        self.assertRefused(self.check(), "no host renderer attestation")
        self.assertRefused(self.check([]), "records no renderer context")

    def test_a_record_without_strings_is_refused(self):
        self.assertRefused(self.check([record("context-create", None)]), "no GL renderer strings")

    def test_another_format_is_refused(self):
        self.assertRefused(self.check(workload(), fmt=2), "not format 1")

    def test_creation_without_a_submission_is_refused(self):
        self.assertRefused(self.check([record("context-create", METAL)]), "records no first-submit-3d")
        self.assertRefused(self.check([record("context-create", METAL),
                                       record("context-create", METAL, ctx_id=2)]),
                           "records no first-submit-3d")

    def test_a_submission_needs_its_context_created_first(self):
        self.assertRefused(self.check([record("first-submit-3d", METAL), record("context-create", METAL)]),
                           "first-submit-3d of context 1 has no context-create")
        self.assertRefused(self.check([record("context-create", METAL),
                                       record("first-submit-3d", METAL, ctx_id=2)]),
                           "first-submit-3d of context 2 has no context-create")

    def test_a_repeated_first_submission_is_refused(self):
        self.assertRefused(self.check(workload() + [record("first-submit-3d", METAL)]),
                           "repeats that context's first submission")

    def test_a_recreated_context_may_submit_again(self):
        result = self.check(workload() + workload())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unknown_events_and_bad_context_ids_are_refused(self):
        self.assertRefused(self.check(workload() + [record("context-destroy", METAL)]),
                           "unknown event 'context-destroy'")
        for ctx in (0, -1, "1", None, True):
            with self.subTest(ctx=ctx):
                self.assertRefused(self.check([record("context-create", METAL, ctx_id=ctx)]),
                                   "has no valid ctx_id")

    def test_software_renderers_are_refused_by_name(self):
        for renderer, name in (("llvmpipe (LLVM 22.1.8, 128 bits)", "llvmpipe"),
                               ("ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero)), SwiftShader driver)",
                                "SwiftShader"),
                               ("Apple Software Renderer", "Software")):
            with self.subTest(renderer=renderer):
                self.assertRefused(self.check([record("context-create", METAL),
                                               record("first-submit-3d", renderer)]),
                                   f"software renderer, {name}")

    def test_another_hardware_backend_is_refused(self):
        other = "ANGLE (Apple, Apple M1, OpenGL 4.1 Metal - 90.5)"
        self.assertRefused(self.check(workload(other)), "not the Apple GPU through ANGLE's Metal")

    def test_a_guest_capset_naming_another_renderer_is_refused(self):
        self.assertRefused(self.check(workload(), serial="VIRGL_CAPSET_RENDERER=llvmpipe\n"),
                           "capset renderer 'llvmpipe'")


class BootInteractiveGpuGlTest(unittest.TestCase):
    """Runs the launcher with stub host services and inspects the QEMU command line it builds."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="gl-", dir="/tmp")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(ROOT / "scripts/boot-interactive.sh", scripts / "boot-interactive.sh")
        shutil.copyfile(CHECK, scripts / "check-host-renderer.py")
        (scripts / "host-slots.sh").write_text("\n".join(
            f"{name}() {{ :; }}" for name in (
                "host_slots_start", "host_slot_acquire", "host_slot_serial",
                "host_slot_quiet", "host_slot_header")))
        lock = self.root / "docker/qemu/lib/qemu-host-lock.sh"
        lock.parent.mkdir(parents=True)
        lock.write_text("qemu_host_lock_acquire() { :; }\nqemu_host_lock_track_pid() { :; }\n")
        self.executable(scripts / "vigil-record.sh", "#!/bin/sh\nexit 0\n")
        self.executable(scripts / "qemu-1x-app.sh", "#!/bin/sh\necho qemu-system-aarch64\n")
        self.executable(scripts / "build-qemu-gl.sh",
                        '#!/bin/sh\n[ "$1" = --print-path ] && [ -n "$STUB_GL" ] || exit 1\necho "$STUB_GL"\n')
        (scripts / "qemu-arm64.py").write_text('print("default")\n')
        # The stub VM writes the attestation STUB_RECORD names (none when it is empty) and exits 0.
        (scripts / "watch-qemu.py").write_text(
            "import json, os, pathlib, sys\n"
            "pathlib.Path(os.environ['CAPTURE']).write_text(json.dumps({\n"
            "    'argv': sys.argv[sys.argv.index('--') + 1:],\n"
            "    'angle': os.environ.get('ANGLE_DEFAULT_PLATFORM'),\n"
            "    'record': os.environ.get('BREENIX_HOST_RENDERER_JSON')}))\n"
            "if os.environ.get('STUB_RECORD'):\n"
            "    pathlib.Path(os.environ['BREENIX_HOST_RENDERER_JSON']).write_text(os.environ['STUB_RECORD'])\n")
        kernel = self.root / "target/aarch64-breenix-kernel/release/kernel-aarch64"
        kernel.parent.mkdir(parents=True)
        kernel.touch()
        (self.root / "target/ext2-aarch64.img").touch()

    @staticmethod
    def executable(path, text):
        path.write_text(text)
        path.chmod(0o755)

    def launch(self, *args, gl="/stub/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64", records=None):
        env = {k: v for k, v in os.environ.items() if k not in ("ANGLE_DEFAULT_PLATFORM", "BREENIX_QEMU_NOGUI")}
        env.update(CAPTURE=str(self.root / "launch.json"), STUB_GL=gl or "",
                   STUB_RECORD=json.dumps({"format": 1, "pid": 1, "records": records}) if records is not None else "")
        result = subprocess.run(
            ["bash", str(self.root / "scripts/boot-interactive.sh"), "--no-build", "--mode", "probe",
             "--serial-log", str(self.root / "run/serial.txt"), *args],
            cwd=self.root, env=env, capture_output=True, text=True, timeout=20)
        launch = json.loads((self.root / "launch.json").read_text()) if (self.root / "launch.json").exists() else None
        return result, launch

    def test_the_window_uses_cocoa_gl_on_the_gl_binary_with_the_gl_device(self):
        result, launch = self.launch("--gpu-gl", "--display", records=workload())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        argv = launch["argv"]
        self.assertEqual(argv[2], "/stub/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64")
        self.assertEqual(argv[argv.index("-display") + 1], "cocoa,gl=es")
        self.assertNotIn("-spice", argv)
        # Ruling 21 (#1697): PRESENT must update console 0 in the watched window,
        # so the GL PCI display is registered before the MMIO framebuffer display.
        self.assertEqual(argv[argv.index("virtio-gpu-gl-pci") + 1:argv.index("virtio-gpu-gl-pci") + 3],
                         ["-device", "virtio-gpu-device"])
        self.assertEqual(launch["angle"], "metal")
        self.assertEqual(launch["record"], str(self.root / "run/host-renderer.json"))
        self.assertIn("host renderer: accepted", result.stdout)

    def test_a_run_without_a_window_has_spice_gl_and_no_display(self):
        result, launch = self.launch("--gpu-gl", "--no-display", records=workload())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        argv = launch["argv"]
        self.assertEqual(argv[argv.index("-display") + 1], "none")
        spice = argv[argv.index("-spice") + 1]
        self.assertTrue(spice.startswith("unix=on,addr=/tmp/breenix-spice-"), spice)
        self.assertTrue(spice.endswith(",disable-ticketing=on,gl=es"), spice)
        self.assertIn("virtio-gpu-gl-pci", argv)

    def test_a_refused_attestation_fails_the_boot(self):
        result, _ = self.launch("--gpu-gl", "--no-display",
                                records=workload("llvmpipe (LLVM 22.1.8, 128 bits)"))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("REFUSED", result.stdout)
        result, _ = self.launch("--gpu-gl", "--no-display")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("no host renderer attestation", result.stdout)

    def test_a_named_gl_binary_replaces_the_installed_one(self):
        os.environ["BREENIX_QEMU_GL_BINARY"] = "/planted/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64"
        self.addCleanup(os.environ.pop, "BREENIX_QEMU_GL_BINARY")
        result, launch = self.launch("--gpu-gl", "--no-display", gl=None, records=workload())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(launch["argv"][2], "/planted/QEMU-GL.app/Contents/MacOS/qemu-system-aarch64")

    def test_without_the_gl_binary_the_launcher_stops_before_booting(self):
        result, launch = self.launch("--gpu-gl", "--display", gl=None)
        self.assertEqual(result.returncode, 1)
        self.assertIn("run scripts/build-qemu-gl.sh", result.stderr)
        self.assertIsNone(launch)

    def test_without_the_option_nothing_changes(self):
        result, launch = self.launch("--no-display")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("virtio-gpu-gl-pci", launch["argv"])
        self.assertNotIn("-spice", launch["argv"])
        self.assertIsNone(launch["record"])
        self.assertNotIn("host renderer", result.stdout)


if __name__ == "__main__":
    unittest.main()
