#!/usr/bin/env python3
"""
ARM64 Visual Terminal Testing via VNC

Alternative approach using vncdotool for VNC-based interaction.
This provides richer interaction capabilities when a VNC display is available.

Benefits over QEMU monitor approach:
- Can capture screen directly via VNC protocol
- More natural keyboard interaction
- Can detect screen changes in real-time

Dependencies:
    pip install vncdotool pillow pytesseract

System Dependencies:
    - Tesseract OCR: brew install tesseract (macOS)
    - QEMU: qemu-system-aarch64

Usage:
    ./scripts/test-arm64-vnc.py              # Full test
    ./scripts/test-arm64-vnc.py --no-build   # Skip kernel build
    ./scripts/test-arm64-vnc.py --verbose    # Show detailed output
    ./scripts/test-arm64-vnc.py --keep-screens # Keep screenshots

Exit codes:
    0 = SUCCESS
    1 = FAILURE
    2 = SKIP (missing dependencies)
"""

import argparse
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional, Tuple, List

# Try to import optional dependencies
try:
    from vncdotool import api as vnc_api
    HAVE_VNCDOTOOL = True
except ImportError:
    HAVE_VNCDOTOOL = False

try:
    from PIL import Image
    HAVE_PIL = True
except ImportError:
    HAVE_PIL = False

try:
    import pytesseract
    HAVE_TESSERACT = True
except ImportError:
    HAVE_TESSERACT = False


def get_project_root() -> Path:
    """Get the Breenix project root directory."""
    script_dir = Path(__file__).parent.absolute()
    return script_dir.parent


def check_dependencies() -> Tuple[bool, List[str]]:
    """Check for required dependencies."""
    missing = []

    if not HAVE_VNCDOTOOL:
        missing.append("vncdotool (pip install vncdotool)")

    if not HAVE_PIL:
        missing.append("pillow (pip install pillow)")

    if not HAVE_TESSERACT:
        missing.append("pytesseract (pip install pytesseract)")
    else:
        try:
            subprocess.run(["tesseract", "--version"],
                          capture_output=True, check=True)
        except (subprocess.CalledProcessError, FileNotFoundError):
            missing.append("tesseract binary (brew install tesseract)")

    try:
        subprocess.run(["qemu-system-aarch64", "--version"],
                      capture_output=True, check=True)
    except (subprocess.CalledProcessError, FileNotFoundError):
        missing.append("qemu-system-aarch64")

    return len(missing) == 0, missing


def build_kernel(project_root: Path) -> bool:
    """Build the ARM64 kernel."""
    print("[1/5] Building ARM64 kernel...")

    cmd = [
        "cargo", "build", "--release",
        "--target", "aarch64-breenix.json",
        "-Z", "build-std=core,alloc",
        "-Z", "build-std-features=compiler-builtins-mem",
        "-p", "kernel",
        "--bin", "kernel-aarch64"
    ]

    result = subprocess.run(cmd, cwd=project_root, capture_output=True, text=True)

    if result.returncode != 0:
        print("ERROR: Build failed")
        print(result.stderr)
        return False

    kernel_path = project_root / "target/aarch64-breenix/release/kernel-aarch64"
    if not kernel_path.exists():
        print(f"ERROR: Kernel not found at {kernel_path}")
        return False

    print(f"Kernel built: {kernel_path}")
    return True


def extract_text_from_image(image_path: str) -> str:
    """Extract text from image using OCR."""
    if not HAVE_PIL or not HAVE_TESSERACT:
        return ""

    try:
        img = Image.open(image_path)
        img = img.convert('L')  # Grayscale
        text = pytesseract.image_to_string(img, config='--psm 6')
        return text
    except Exception as e:
        print(f"OCR error: {e}")
        return ""


class VNCVisualTest:
    """ARM64 visual test using VNC."""

    def __init__(self, project_root: Path, verbose: bool = False,
                 keep_screens: bool = False):
        self.project_root = project_root
        self.verbose = verbose
        self.keep_screens = keep_screens
        self.qemu_process: Optional[subprocess.Popen] = None
        self.vnc_client = None
        self.temp_dir = tempfile.mkdtemp(prefix="breenix_vnc_")
        self.serial_file = os.path.join(self.temp_dir, "serial.txt")
        self.screen_count = 0
        self.vnc_port = 5900

    def start_qemu(self) -> bool:
        """Start QEMU with VNC display."""
        print("[2/5] Starting QEMU with VNC...")

        subprocess.run(
            ["pkill", "-9", "-f", "qemu-system-aarch64.*kernel-aarch64"],
            capture_output=True
        )
        time.sleep(0.5)

        kernel_path = self.project_root / "target/aarch64-breenix/release/kernel-aarch64"
        ext2_disk = self.project_root / "target/ext2-aarch64.img"

        cmd = [
            "qemu-system-aarch64",
            "-M", "virt",
            "-cpu", "cortex-a72",
            "-m", "512M",
            "-vnc", ":0",  # VNC on display :0 (port 5900)
            "-device", "virtio-gpu-device",
            "-device", "virtio-keyboard-device",
            "-kernel", str(kernel_path),
            "-serial", f"file:{self.serial_file}",
            "-no-reboot",
        ]

        if ext2_disk.exists():
            cmd.extend([
                "-device", "virtio-blk-device,drive=ext2disk",
                "-blockdev", f"driver=file,node-name=ext2file,filename={ext2_disk}",
                "-blockdev", "driver=raw,node-name=ext2disk,file=ext2file",
            ])

        cmd.extend([
            "-device", "virtio-net-device,netdev=net0",
            "-netdev", "user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15",
        ])

        if self.verbose:
            print(f"QEMU command: {' '.join(cmd)}")

        self.qemu_process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )

        return True

    def connect_vnc(self, timeout: float = 15.0) -> bool:
        """Connect to VNC server."""
        print("[3/5] Connecting to VNC...")

        if not HAVE_VNCDOTOOL:
            print("ERROR: vncdotool not available")
            return False

        start = time.time()
        while time.time() - start < timeout:
            try:
                self.vnc_client = vnc_api.connect(f"localhost::{self.vnc_port}")
                print("Connected to VNC")
                return True
            except Exception as e:
                if self.verbose:
                    print(f"VNC connection attempt failed: {e}")
                time.sleep(0.5)

        print(f"TIMEOUT connecting to VNC ({timeout}s)")
        return False

    def wait_for_boot(self, timeout: float = 30.0) -> bool:
        """Wait for kernel to boot."""
        print("[4/5] Waiting for boot...")

        start = time.time()
        while time.time() - start < timeout:
            if os.path.exists(self.serial_file):
                try:
                    with open(self.serial_file, 'r') as f:
                        content = f.read()

                    if "breenix>" in content:
                        print("Shell prompt detected")
                        return True
                except Exception:
                    pass

            time.sleep(0.2)

        print(f"TIMEOUT waiting for boot ({timeout}s)")
        return False

    def capture_screen(self, name: str = None) -> Tuple[str, str]:
        """Capture screen via VNC and return (path, text)."""
        if not self.vnc_client:
            return "", ""

        self.screen_count += 1
        if name:
            filename = f"vnc_screen_{self.screen_count:03d}_{name}.png"
        else:
            filename = f"vnc_screen_{self.screen_count:03d}.png"

        image_path = os.path.join(self.temp_dir, filename)

        try:
            self.vnc_client.captureScreen(image_path)
            text = extract_text_from_image(image_path)

            if self.verbose:
                print(f"Captured: {image_path}")
                print(f"OCR text: {text[:200]}...")

            return image_path, text
        except Exception as e:
            print(f"Screen capture error: {e}")
            return "", ""

    def send_key(self, key: str, delay_ms: int = 50) -> None:
        """Send a key via VNC."""
        if self.vnc_client:
            try:
                self.vnc_client.keyPress(key)
                time.sleep(delay_ms / 1000.0)
            except Exception as e:
                if self.verbose:
                    print(f"Key press error: {e}")

    def send_string(self, text: str, delay_ms: int = 50) -> None:
        """Send string via VNC."""
        if not self.vnc_client:
            return

        for char in text:
            try:
                self.vnc_client.keyPress(char)
                time.sleep(delay_ms / 1000.0)
            except Exception:
                pass

    def test_keyboard_input(self) -> bool:
        """Test keyboard input via VNC."""
        print("[5/5] Testing keyboard input...")

        if not self.vnc_client:
            return False

        time.sleep(1.0)

        # Capture initial
        _, initial = self.capture_screen("initial")

        # Type test command
        test_string = "echo VNCTEST"
        print(f"  Sending: {test_string}")
        self.send_string(test_string)

        time.sleep(0.5)
        _, typed = self.capture_screen("typed")

        # Press Enter
        print("  Sending: Enter")
        self.send_key("enter")

        time.sleep(1.0)
        _, result = self.capture_screen("result")

        # Check result
        serial_text = ""
        if os.path.exists(self.serial_file):
            with open(self.serial_file, 'r') as f:
                serial_text = f.read()

        if "VNCTEST" in result.upper():
            print("  PASS: Output visible on screen (OCR)")
            return True
        elif "VNCTEST" in serial_text:
            print("  PASS: Output in serial (keyboard works)")
            return True
        else:
            print("  FAIL: Could not verify output")
            return False

    def cleanup(self):
        """Clean up."""
        if self.vnc_client:
            try:
                self.vnc_client.disconnect()
            except Exception:
                pass
            self.vnc_client = None

        if self.qemu_process:
            try:
                self.qemu_process.terminate()
                self.qemu_process.wait(timeout=5)
            except Exception:
                try:
                    self.qemu_process.kill()
                except Exception:
                    pass

        subprocess.run(
            ["pkill", "-9", "-f", "qemu-system-aarch64.*kernel-aarch64"],
            capture_output=True
        )

        if not self.keep_screens:
            import shutil
            try:
                shutil.rmtree(self.temp_dir)
            except Exception:
                pass
        else:
            print(f"\nScreenshots saved in: {self.temp_dir}")

    def get_serial_output(self) -> str:
        """Get serial output."""
        if os.path.exists(self.serial_file):
            with open(self.serial_file, 'r') as f:
                return f.read()
        return ""


def run_test(args) -> int:
    """Run the VNC visual test."""
    project_root = get_project_root()

    print("========================================")
    print("  ARM64 VNC Visual Terminal Test")
    print("========================================")
    print()

    ok, missing = check_dependencies()
    if not ok:
        print("Missing dependencies:")
        for dep in missing:
            print(f"  - {dep}")
        print()
        print("Install with:")
        print("  pip install vncdotool pillow pytesseract")
        return 2

    print("Dependencies OK")
    print()

    if not args.no_build:
        if not build_kernel(project_root):
            return 1
    else:
        print("[1/5] Skipping build (--no-build)")

    kernel_path = project_root / "target/aarch64-breenix/release/kernel-aarch64"
    if not kernel_path.exists():
        print(f"ERROR: Kernel not found at {kernel_path}")
        return 1

    print()

    test = VNCVisualTest(
        project_root,
        verbose=args.verbose,
        keep_screens=args.keep_screens
    )

    try:
        if not test.start_qemu():
            return 1

        if not test.connect_vnc():
            return 1

        if not test.wait_for_boot(timeout=args.boot_timeout):
            return 1

        if test.test_keyboard_input():
            print()
            print("========================================")
            print("SUCCESS: VNC visual test passed!")
            print("========================================")
            return 0
        else:
            print()
            print("FAIL: VNC visual test failed")
            return 1

    finally:
        test.cleanup()


def main():
    parser = argparse.ArgumentParser(
        description="Test ARM64 visual terminal via VNC",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )

    parser.add_argument("--no-build", action="store_true",
                       help="Skip kernel build")
    parser.add_argument("--verbose", "-v", action="store_true",
                       help="Show detailed output")
    parser.add_argument("--boot-timeout", type=int, default=30,
                       help="Boot timeout in seconds")
    parser.add_argument("--keep-screens", action="store_true",
                       help="Keep screenshot files")

    args = parser.parse_args()
    return run_test(args)


if __name__ == "__main__":
    sys.exit(main())
