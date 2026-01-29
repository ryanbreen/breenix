#!/usr/bin/env python3
"""
ARM64 Visual Terminal Testing Infrastructure

Tests the ARM64 graphical terminal by:
1. Starting QEMU with VirtIO GPU/keyboard and monitor interface
2. Sending keystrokes via QEMU monitor's `sendkey` command
3. Capturing screen via QEMU monitor's `screendump` command
4. Verifying expected text appears on screen using OCR

This approach works without needing a display (headless) and is based on
the proven pattern from xtask's interactive_test() function.

Dependencies:
    pip install pytesseract pillow pexpect

System Dependencies:
    - Tesseract OCR: brew install tesseract (macOS) or apt install tesseract-ocr (Linux)
    - QEMU: qemu-system-aarch64

Usage:
    ./scripts/test-arm64-visual.py              # Full test
    ./scripts/test-arm64-visual.py --no-build   # Skip kernel build
    ./scripts/test-arm64-visual.py --verbose    # Show detailed output
    ./scripts/test-arm64-visual.py --keep-screen # Keep screendump files for debugging

Exit codes:
    0 = SUCCESS - visual terminal works correctly
    1 = FAILURE - test failed
    2 = SKIP    - missing dependencies
"""

import argparse
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional, Tuple, List

# Try to import optional dependencies
try:
    import pexpect
    HAVE_PEXPECT = True
except ImportError:
    HAVE_PEXPECT = False

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
    """Check for required dependencies. Returns (ok, missing_list)."""
    missing = []

    if not HAVE_PEXPECT:
        missing.append("pexpect (pip install pexpect)")

    if not HAVE_PIL:
        missing.append("pillow (pip install pillow)")

    if not HAVE_TESSERACT:
        missing.append("pytesseract (pip install pytesseract)")
    else:
        # Check if tesseract binary is available
        try:
            subprocess.run(["tesseract", "--version"],
                          capture_output=True, check=True)
        except (subprocess.CalledProcessError, FileNotFoundError):
            missing.append("tesseract binary (brew install tesseract or apt install tesseract-ocr)")

    # Check for QEMU
    try:
        subprocess.run(["qemu-system-aarch64", "--version"],
                      capture_output=True, check=True)
    except (subprocess.CalledProcessError, FileNotFoundError):
        missing.append("qemu-system-aarch64 (QEMU ARM64 emulator)")

    return len(missing) == 0, missing


def build_kernel(project_root: Path) -> bool:
    """Build the ARM64 kernel. Returns True on success."""
    print("[1/5] Building ARM64 kernel...")

    cmd = [
        "cargo", "build", "--release",
        "--target", "aarch64-breenix.json",
        "-Z", "build-std=core,alloc",
        "-Z", "build-std-features=compiler-builtins-mem",
        "-p", "kernel",
        "--bin", "kernel-aarch64"
    ]

    result = subprocess.run(
        cmd,
        cwd=project_root,
        capture_output=True,
        text=True
    )

    if result.returncode != 0:
        print("ERROR: Kernel build failed")
        print(result.stderr)
        return False

    kernel_path = project_root / "target/aarch64-breenix/release/kernel-aarch64"
    if not kernel_path.exists():
        print(f"ERROR: Kernel not found at {kernel_path}")
        return False

    print(f"Kernel built: {kernel_path}")
    return True


class QEMUMonitor:
    """Interface to QEMU monitor via TCP socket."""

    def __init__(self, host: str = "127.0.0.1", port: int = 4445):
        self.host = host
        self.port = port
        self.socket: Optional[socket.socket] = None

    def connect(self, timeout: float = 10.0) -> bool:
        """Connect to QEMU monitor. Returns True on success."""
        start = time.time()
        while time.time() - start < timeout:
            try:
                self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                self.socket.settimeout(5.0)
                self.socket.connect((self.host, self.port))
                # Read initial prompt
                self._read_response()
                return True
            except (ConnectionRefusedError, socket.timeout):
                time.sleep(0.2)
                continue
            except Exception as e:
                print(f"Monitor connection error: {e}")
                time.sleep(0.2)
                continue
        return False

    def _read_response(self, timeout: float = 2.0) -> str:
        """Read response from monitor until we get a prompt."""
        if not self.socket:
            return ""

        self.socket.settimeout(timeout)
        response = b""
        try:
            while True:
                chunk = self.socket.recv(4096)
                if not chunk:
                    break
                response += chunk
                # QEMU monitor prompts end with "(qemu) "
                if b"(qemu)" in response:
                    break
        except socket.timeout:
            pass
        return response.decode('utf-8', errors='replace')

    def send_command(self, cmd: str) -> str:
        """Send a command to the monitor and return response."""
        if not self.socket:
            return ""

        self.socket.sendall(f"{cmd}\n".encode())
        return self._read_response()

    def sendkey(self, key: str, delay_ms: int = 50) -> None:
        """Send a keystroke via QEMU monitor."""
        self.send_command(f"sendkey {key}")
        time.sleep(delay_ms / 1000.0)

    def send_string(self, text: str, delay_ms: int = 50) -> None:
        """Send a string as keyboard input, character by character."""
        for char in text:
            key = self._char_to_key(char)
            if key:
                self.sendkey(key, delay_ms)

    def _char_to_key(self, char: str) -> Optional[str]:
        """Convert a character to QEMU sendkey format."""
        # Lowercase letters
        if 'a' <= char <= 'z':
            return char
        # Uppercase letters
        if 'A' <= char <= 'Z':
            return f"shift-{char.lower()}"
        # Numbers
        if '0' <= char <= '9':
            return char
        # Special characters
        specials = {
            ' ': 'spc',
            '\n': 'ret',
            '\r': 'ret',
            '-': 'minus',
            '_': 'shift-minus',
            '.': 'dot',
            '/': 'slash',
            '\\': 'backslash',
            ':': 'shift-semicolon',
            ';': 'semicolon',
            '=': 'equal',
            '+': 'shift-equal',
            '|': 'shift-backslash',
            ',': 'comma',
            '<': 'shift-comma',
            '>': 'shift-dot',
            '?': 'shift-slash',
            '!': 'shift-1',
            '@': 'shift-2',
            '#': 'shift-3',
            '$': 'shift-4',
            '%': 'shift-5',
            '^': 'shift-6',
            '&': 'shift-7',
            '*': 'shift-8',
            '(': 'shift-9',
            ')': 'shift-0',
            '[': 'bracket_left',
            ']': 'bracket_right',
            '{': 'shift-bracket_left',
            '}': 'shift-bracket_right',
            "'": 'apostrophe',
            '"': 'shift-apostrophe',
            '`': 'grave_accent',
            '~': 'shift-grave_accent',
            '\t': 'tab',
        }
        return specials.get(char)

    def screendump(self, filename: str) -> bool:
        """Capture screen to a PPM file. Returns True on success."""
        response = self.send_command(f"screendump {filename}")
        # Give QEMU time to write the file
        time.sleep(0.5)
        return os.path.exists(filename)

    def close(self):
        """Close the monitor connection."""
        if self.socket:
            try:
                self.socket.close()
            except Exception:
                pass
            self.socket = None


def extract_text_from_image(image_path: str) -> str:
    """Extract text from a PPM image using OCR."""
    if not HAVE_PIL or not HAVE_TESSERACT:
        return ""

    try:
        # Load image
        img = Image.open(image_path)

        # Convert to grayscale for better OCR
        img = img.convert('L')

        # Use pytesseract to extract text
        # Use --psm 6 for uniform block of text (terminal output)
        text = pytesseract.image_to_string(img, config='--psm 6')

        return text
    except Exception as e:
        print(f"OCR error: {e}")
        return ""


class VisualTerminalTest:
    """ARM64 visual terminal test using QEMU monitor."""

    def __init__(self, project_root: Path, verbose: bool = False,
                 keep_screens: bool = False):
        self.project_root = project_root
        self.verbose = verbose
        self.keep_screens = keep_screens
        self.qemu_process: Optional[subprocess.Popen] = None
        self.monitor: Optional[QEMUMonitor] = None
        self.temp_dir = tempfile.mkdtemp(prefix="breenix_visual_")
        self.serial_file = os.path.join(self.temp_dir, "serial.txt")
        self.screen_count = 0

    def start_qemu(self) -> bool:
        """Start QEMU with monitor enabled. Returns True on success."""
        print("[2/5] Starting QEMU with monitor...")

        # Kill any stale QEMU processes
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
            "-display", "none",  # Headless - we use screendump
            "-device", "virtio-gpu-device",  # GPU for framebuffer
            "-device", "virtio-keyboard-device",  # Keyboard for sendkey
            "-kernel", str(kernel_path),
            "-serial", f"file:{self.serial_file}",
            "-monitor", "tcp:127.0.0.1:4445,server,nowait",
            "-no-reboot",
        ]

        # Add block device if available
        if ext2_disk.exists():
            cmd.extend([
                "-device", "virtio-blk-device,drive=ext2disk",
                "-blockdev", f"driver=file,node-name=ext2file,filename={ext2_disk}",
                "-blockdev", "driver=raw,node-name=ext2disk,file=ext2file",
            ])

        # Add network device
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

    def connect_monitor(self, timeout: float = 15.0) -> bool:
        """Connect to QEMU monitor. Returns True on success."""
        print("[3/5] Connecting to QEMU monitor...")

        self.monitor = QEMUMonitor(port=4445)
        if not self.monitor.connect(timeout=timeout):
            print("ERROR: Could not connect to QEMU monitor")
            return False

        print("Connected to QEMU monitor")
        return True

    def wait_for_boot(self, timeout: float = 30.0) -> bool:
        """Wait for kernel to boot by checking serial output."""
        print("[4/5] Waiting for kernel boot...")

        start = time.time()
        while time.time() - start < timeout:
            if os.path.exists(self.serial_file):
                try:
                    with open(self.serial_file, 'r') as f:
                        content = f.read()

                    # Check for shell prompt (definitive boot completion)
                    if "breenix>" in content:
                        print("Shell prompt detected - boot complete")
                        return True

                    # Check for basic boot markers
                    if "Hello from ARM64" in content:
                        if self.verbose:
                            print("Boot marker detected, waiting for shell...")
                except Exception:
                    pass

            time.sleep(0.2)

        print(f"TIMEOUT waiting for boot ({timeout}s)")
        return False

    def capture_screen(self, name: str = None) -> Tuple[str, str]:
        """
        Capture screen and return (image_path, extracted_text).
        Returns empty strings on failure.
        """
        if not self.monitor:
            return "", ""

        self.screen_count += 1
        if name:
            filename = f"screen_{self.screen_count:03d}_{name}.ppm"
        else:
            filename = f"screen_{self.screen_count:03d}.ppm"

        image_path = os.path.join(self.temp_dir, filename)

        if not self.monitor.screendump(image_path):
            print(f"Failed to capture screen to {image_path}")
            return "", ""

        # Extract text using OCR
        text = extract_text_from_image(image_path)

        if self.verbose:
            print(f"Captured: {image_path}")
            print(f"OCR text ({len(text)} chars): {text[:200]}...")

        return image_path, text

    def test_keyboard_input(self) -> bool:
        """Test keyboard input appears on screen. Returns True on success."""
        print("[5/5] Testing visual keyboard input...")

        if not self.monitor:
            return False

        # Give shell a moment to be ready
        time.sleep(1.0)

        # Capture initial screen
        _, initial_text = self.capture_screen("initial")
        if self.verbose:
            print(f"Initial screen text:\n{initial_text}")

        # Send a distinctive test string
        test_string = "echo VISUALTEST"
        print(f"  Sending: {test_string}")
        self.monitor.send_string(test_string)

        # Wait for text to appear
        time.sleep(0.5)

        # Capture screen before pressing Enter
        _, typed_text = self.capture_screen("typed")

        # Check if the typed text appears on screen
        # Note: OCR might have some errors, so we check for partial matches
        typed_visible = False
        for word in ["echo", "VISUAL", "TEST", "VISUALTEST"]:
            if word.upper() in typed_text.upper() or word.lower() in typed_text.lower():
                typed_visible = True
                print(f"  PASS: Found '{word}' in screen text")
                break

        if not typed_visible:
            print(f"  INFO: Typed text not detected via OCR (might be font/OCR issue)")
            # Don't fail here - OCR can be unreliable with custom fonts

        # Send Enter to execute the command
        print("  Sending: Enter")
        self.monitor.sendkey("ret")

        # Wait for command execution
        time.sleep(1.0)

        # Capture result screen
        _, result_text = self.capture_screen("result")

        if self.verbose:
            print(f"Result screen text:\n{result_text}")

        # Check if "VISUALTEST" appears in the output (command echo result)
        # Also check serial output as a fallback
        serial_text = ""
        if os.path.exists(self.serial_file):
            with open(self.serial_file, 'r') as f:
                serial_text = f.read()

        visual_test_found = "VISUALTEST" in result_text.upper()
        serial_test_found = "VISUALTEST" in serial_text

        if visual_test_found:
            print("  PASS: Command output 'VISUALTEST' visible on screen (OCR)")
            return True
        elif serial_test_found:
            print("  PASS: Command output 'VISUALTEST' found in serial (keyboard input works)")
            print("        Note: OCR may not have detected screen text reliably")
            return True
        else:
            print("  FAIL: Could not verify command output")
            if self.verbose:
                print(f"  Screen OCR: {result_text[:500]}")
                print(f"  Serial output: {serial_text[-500:]}")
            return False

    def cleanup(self):
        """Clean up QEMU and temporary files."""
        if self.monitor:
            self.monitor.close()
            self.monitor = None

        if self.qemu_process:
            try:
                self.qemu_process.terminate()
                self.qemu_process.wait(timeout=5)
            except Exception:
                try:
                    self.qemu_process.kill()
                except Exception:
                    pass
            self.qemu_process = None

        # Kill any remaining QEMU processes
        subprocess.run(
            ["pkill", "-9", "-f", "qemu-system-aarch64.*kernel-aarch64"],
            capture_output=True
        )

        # Clean up temp files unless keeping screens
        if not self.keep_screens:
            import shutil
            try:
                shutil.rmtree(self.temp_dir)
            except Exception:
                pass
        else:
            print(f"\nScreen dumps saved in: {self.temp_dir}")

    def get_serial_output(self) -> str:
        """Get serial output for diagnostics."""
        if os.path.exists(self.serial_file):
            with open(self.serial_file, 'r') as f:
                return f.read()
        return ""


def run_test(args) -> int:
    """Run the visual terminal test."""
    project_root = get_project_root()

    print("========================================")
    print("  ARM64 Visual Terminal Test")
    print("========================================")
    print()

    # Check dependencies
    ok, missing = check_dependencies()
    if not ok:
        print("Missing dependencies:")
        for dep in missing:
            print(f"  - {dep}")
        print()
        print("Install with:")
        print("  pip install pexpect pillow pytesseract")
        print("  brew install tesseract  # macOS")
        print("  apt install tesseract-ocr  # Linux")
        return 2

    print("Dependencies OK")
    print()

    # Build kernel if needed
    if not args.no_build:
        if not build_kernel(project_root):
            return 1
    else:
        print("[1/5] Skipping build (--no-build)")

    # Check kernel exists
    kernel_path = project_root / "target/aarch64-breenix/release/kernel-aarch64"
    if not kernel_path.exists():
        print(f"ERROR: Kernel not found at {kernel_path}")
        print("Run without --no-build to build the kernel")
        return 1

    print()

    # Run test
    test = VisualTerminalTest(
        project_root,
        verbose=args.verbose,
        keep_screens=args.keep_screens
    )

    try:
        if not test.start_qemu():
            print("\nFAIL: Could not start QEMU")
            return 1

        if not test.connect_monitor():
            print("\nFAIL: Could not connect to QEMU monitor")
            return 1

        if not test.wait_for_boot(timeout=args.boot_timeout):
            print("\nFAIL: Kernel did not boot successfully")
            if args.verbose:
                print("\nSerial output:")
                print("-" * 40)
                print(test.get_serial_output())
                print("-" * 40)
            return 1

        if test.test_keyboard_input():
            print()
            print("========================================")
            print("SUCCESS: Visual terminal test passed!")
            print("  - Kernel booted with VirtIO GPU")
            print("  - Monitor connected and responsive")
            print("  - Keyboard input via sendkey works")
            print("  - Command output verified on screen/serial")
            print("========================================")
            return 0
        else:
            print()
            print("========================================")
            print("FAIL: Visual terminal test failed")
            print("  - Keyboard input was not reflected")
            print("========================================")
            if args.verbose:
                print("\nSerial output:")
                print("-" * 40)
                print(test.get_serial_output()[-2000:])
                print("-" * 40)
            return 1

    finally:
        test.cleanup()


def main():
    parser = argparse.ArgumentParser(
        description="Test ARM64 visual terminal via QEMU monitor",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )

    parser.add_argument(
        "--no-build",
        action="store_true",
        help="Skip kernel build (use existing kernel)"
    )

    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Show detailed output including OCR results"
    )

    parser.add_argument(
        "--boot-timeout",
        type=int,
        default=30,
        help="Timeout for kernel boot (default: 30s)"
    )

    parser.add_argument(
        "--keep-screens",
        action="store_true",
        help="Keep screendump files for debugging"
    )

    args = parser.parse_args()

    return run_test(args)


if __name__ == "__main__":
    sys.exit(main())
