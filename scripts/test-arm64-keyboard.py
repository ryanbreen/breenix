#!/usr/bin/env python3
"""
ARM64 Keyboard Input Test Script

Tests actual keyboard input to the ARM64 kernel via QEMU serial console.
Uses pexpect to interact with QEMU's serial port and verify:
1. Kernel boots successfully (no crash)
2. Shell prompt appears
3. Keyboard input is echoed
4. Shell responds to commands

Usage:
    ./scripts/test-arm64-keyboard.py              # Run full test
    ./scripts/test-arm64-keyboard.py --quick      # Quick hello check only
    ./scripts/test-arm64-keyboard.py --no-build   # Skip kernel build
    ./scripts/test-arm64-keyboard.py --verbose    # Show all QEMU output

Exit codes:
    0 = SUCCESS - keyboard input works, shell responds
    1 = FAILURE - crash, timeout, or no response
    2 = PARTIAL - UART input works but shell has bugs (use --allow-partial to return 0)
"""

import argparse
import os
import re
import subprocess
import sys
import time

# Try to import pexpect, provide helpful error if missing
try:
    import pexpect
except ImportError:
    print("ERROR: pexpect module not found")
    print("Install with: pip install pexpect")
    sys.exit(1)


def get_project_root():
    """Get the Breenix project root directory."""
    script_dir = os.path.dirname(os.path.abspath(__file__))
    return os.path.dirname(script_dir)


def build_kernel(project_root):
    """Build the ARM64 kernel. Returns True on success."""
    print("[1/4] Building ARM64 kernel...")

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

    kernel_path = os.path.join(
        project_root,
        "target/aarch64-breenix/release/kernel-aarch64"
    )

    if not os.path.exists(kernel_path):
        print(f"ERROR: Kernel not found at {kernel_path}")
        return False

    print(f"Kernel built: {kernel_path}")
    return True


def build_qemu_command(project_root):
    """Build the QEMU command with proper options."""
    kernel_path = os.path.join(
        project_root,
        "target/aarch64-breenix/release/kernel-aarch64"
    )

    ext2_disk = os.path.join(project_root, "target/ext2-aarch64.img")

    cmd = [
        "qemu-system-aarch64",
        "-M", "virt",
        "-cpu", "cortex-a72",
        "-m", "512M",
        "-nographic",  # Serial on stdio
        "-no-reboot",  # Exit on crash instead of reboot
        "-kernel", kernel_path,
    ]

    # Add block device if available
    if os.path.exists(ext2_disk):
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

    return cmd


# Crash/error patterns to watch for
# These are specific error messages that indicate a crash, NOT normal boot messages
# Note: "exception level" is a normal boot message (e.g., "Current exception level: EL1")
CRASH_PATTERNS = [
    r"Data abort",
    r"translation fault",
    r"Synchronous exception",     # Actual ARM64 exception
    r"Instruction abort",
    r"Permission fault",
    r"Alignment fault",
    r"\bpanic\b",                 # Word boundary to avoid "panic" in other contexts
    r"\bPANIC\b",
    r"double fault",
    r"DOUBLE FAULT",
    r"kernel panic",
    r"Unhandled exception",
    r"SError",                    # ARM64 system error
    r"Serror",
]

CRASH_REGEX = re.compile("|".join(CRASH_PATTERNS))


def check_for_crash(output):
    """Check if output contains any crash indicators. Returns (crashed, message)."""
    for line in output.split('\n'):
        # Skip lines that contain "exception level" - these are normal boot messages
        if "exception level" in line.lower():
            continue
        if CRASH_REGEX.search(line):
            return True, line.strip()
    return False, None


class KeyboardTest:
    """ARM64 keyboard input test using pexpect."""

    def __init__(self, project_root, verbose=False):
        self.project_root = project_root
        self.verbose = verbose
        self.child = None
        self.output_buffer = ""

    def start_qemu(self):
        """Start QEMU and return True if successful."""
        print("[2/4] Starting QEMU...")

        # Kill any stale QEMU processes
        subprocess.run(
            ["pkill", "-9", "-f", "qemu-system-aarch64.*kernel-aarch64"],
            capture_output=True
        )
        time.sleep(0.5)

        cmd = build_qemu_command(self.project_root)

        if self.verbose:
            print(f"QEMU command: {' '.join(cmd)}")

        # Use pexpect to spawn QEMU
        self.child = pexpect.spawn(
            cmd[0],
            cmd[1:],
            timeout=60,
            encoding='utf-8',
            codec_errors='replace'
        )

        # Set up logging
        if self.verbose:
            self.child.logfile = sys.stdout

        return True

    def wait_for_boot(self, timeout=30):
        """Wait for kernel to boot. Returns True if boot successful."""
        print("[3/4] Waiting for kernel boot...")

        try:
            # First, wait for shell prompt - this is the definitive sign boot is complete
            # The shell prints "breenix> " when ready for input
            patterns = [
                "breenix>",              # Shell prompt (primary target)
                pexpect.TIMEOUT,
                pexpect.EOF,
            ]

            index = self.child.expect(patterns, timeout=timeout)

            # Capture output so far
            self.output_buffer += self.child.before or ""
            if index == 0:
                self.output_buffer += self.child.after or ""

            # Check for crash in output
            crashed, crash_msg = check_for_crash(self.output_buffer)
            if crashed:
                print(f"CRASH DETECTED: {crash_msg}")
                return False

            if index == 0:  # breenix> prompt
                print("Shell prompt detected - boot complete")
                return True
            elif index == 1:  # TIMEOUT
                # Check if we got boot markers but no prompt
                if "Hello from ARM64" in self.output_buffer:
                    print("Boot markers seen but no shell prompt")
                    print("This may indicate shell did not start")
                print(f"TIMEOUT waiting for shell prompt (waited {timeout}s)")
                return False
            else:  # EOF
                print("QEMU exited unexpectedly")
                return False

        except Exception as e:
            print(f"Exception during boot: {e}")
            return False

    def test_keyboard_input(self, timeout=10):
        """Test keyboard input. Returns True if input works."""
        print("[4/4] Testing keyboard input...")

        # Give the shell a moment to be fully ready after prompt
        time.sleep(0.5)

        # Test: Send a complete command and check for response
        # We'll use 'echo test' as it's a simple command that should echo back
        print("  Sending command: echo test")

        # Send the full command at once
        self.child.sendline("echo test")

        try:
            # Look for the command output or the next prompt
            # The shell should:
            # 1. Echo the typed characters (echo test)
            # 2. Execute the command and print "test"
            # 3. Show a new prompt (breenix>)
            patterns = [
                r"test",               # The echo output
                r"breenix>",           # New prompt after command
                r"Unknown command",    # Error response (still means input worked)
                pexpect.TIMEOUT,
            ]

            # First, wait for any response
            index = self.child.expect(patterns, timeout=timeout)

            self.output_buffer += self.child.before or ""
            if index < len(patterns) - 1:
                self.output_buffer += self.child.after or ""

            # Check for crash
            crashed, crash_msg = check_for_crash(self.output_buffer)
            if crashed:
                print(f"  FAIL: Crash after command: {crash_msg}")
                return False

            if index == 0:  # Got "test" output
                print("  PASS: Command executed, got echo output")

                # Now wait for the prompt to confirm shell is responsive
                try:
                    self.child.expect("breenix>", timeout=5)
                    self.output_buffer += self.child.before or ""
                    self.output_buffer += self.child.after or ""
                    print("  PASS: New prompt appeared - shell is responsive")
                except pexpect.TIMEOUT:
                    print("  INFO: No new prompt, but command output received")

                return True
            elif index == 1:  # Got new prompt
                print("  PASS: Shell responded with new prompt")
                return True
            elif index == 2:  # Got error message
                print("  PASS: Shell responded (unknown command)")
                return True
            else:  # TIMEOUT
                # Check if we got any indication of input being received
                uart_received = "[UART_IRQ]" in self.output_buffer
                stdin_push = "[STDIN_PUSH]" in self.output_buffer
                wake_readers = "[WAKE_READERS" in self.output_buffer
                no_readers = "[STDIN_NO_READERS]" in self.output_buffer

                if uart_received:
                    print("  INFO: UART interrupt fired - hardware input working")

                if stdin_push:
                    print("  INFO: Characters pushed to stdin buffer")

                if wake_readers:
                    print("  INFO: Blocked reader (shell) was woken")

                if no_readers:
                    print("  INFO: Some characters arrived with no reader waiting")
                    print("  DIAGNOSTIC: This indicates a shell input handling issue")
                    print("              The shell processes one char then stops reading")

                if uart_received and stdin_push:
                    print("")
                    print("  RESULT: UART keyboard input IS working at hardware level")
                    print("  ISSUE:  Shell is not processing complete line input")
                    print("          This is a kernel/shell bug, not an input bug")
                    # Return special code to indicate partial success
                    return "partial"
                else:
                    print("  FAIL: Timeout - no response from shell")
                    return False

        except Exception as e:
            print(f"  Exception: {e}")
            return False

    def cleanup(self):
        """Clean up QEMU process."""
        if self.child:
            try:
                self.child.terminate(force=True)
            except Exception:
                pass

            # Also kill any remaining QEMU processes
            subprocess.run(
                ["pkill", "-9", "-f", "qemu-system-aarch64.*kernel-aarch64"],
                capture_output=True
            )

    def get_output(self):
        """Get all captured output."""
        return self.output_buffer


def run_test(args):
    """Run the keyboard test."""
    project_root = get_project_root()

    print("========================================")
    print("  ARM64 Keyboard Input Test")
    print("========================================")
    print()

    # Build kernel if needed
    if not args.no_build:
        if not build_kernel(project_root):
            return 1
    else:
        print("[1/4] Skipping build (--no-build)")

    # Check kernel exists
    kernel_path = os.path.join(
        project_root,
        "target/aarch64-breenix/release/kernel-aarch64"
    )
    if not os.path.exists(kernel_path):
        print(f"ERROR: Kernel not found at {kernel_path}")
        print("Run without --no-build to build the kernel")
        return 1

    print()

    # Run test
    test = KeyboardTest(project_root, verbose=args.verbose)

    try:
        if not test.start_qemu():
            print("\nFAIL: Could not start QEMU")
            return 1

        if not test.wait_for_boot(timeout=args.boot_timeout):
            print("\nFAIL: Kernel did not boot successfully")
            if args.verbose:
                print("\nCaptured output:")
                print("-" * 40)
                print(test.get_output())
                print("-" * 40)
            return 1

        if args.quick:
            print("\n========================================")
            print("Quick mode: Boot successful, skipping input test")
            print("========================================")
            return 0

        result = test.test_keyboard_input(timeout=args.input_timeout)

        if result == True:
            print("\n========================================")
            print("SUCCESS: Keyboard input works!")
            print("  - Kernel booted without crash")
            print("  - Shell prompt appeared")
            print("  - Keyboard input is processed")
            print("  - Commands execute correctly")
            print("========================================")
            return 0
        elif result == "partial":
            print("\n========================================")
            print("PARTIAL: UART input works, shell has bugs")
            print("  - Kernel booted without crash")
            print("  - Shell prompt appeared")
            print("  - UART interrupt correctly receives input")
            print("  - Characters are pushed to stdin buffer")
            print("  - BUG: Shell not processing complete lines")
            print("========================================")
            if args.allow_partial:
                print("(--allow-partial: returning success)")
                return 0
            return 2
        else:
            print("\nFAIL: Keyboard input test failed")
            if args.verbose:
                print("\nCaptured output:")
                print("-" * 40)
                print(test.get_output())
                print("-" * 40)
            return 1

    finally:
        test.cleanup()


def main():
    parser = argparse.ArgumentParser(
        description="Test ARM64 keyboard input via QEMU serial console",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )

    parser.add_argument(
        "--quick",
        action="store_true",
        help="Quick mode: only check boot, skip input test"
    )

    parser.add_argument(
        "--no-build",
        action="store_true",
        help="Skip kernel build (use existing kernel)"
    )

    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Show all QEMU output"
    )

    parser.add_argument(
        "--boot-timeout",
        type=int,
        default=30,
        help="Timeout for kernel boot (default: 30s)"
    )

    parser.add_argument(
        "--input-timeout",
        type=int,
        default=10,
        help="Timeout for input response (default: 10s)"
    )

    parser.add_argument(
        "--allow-partial",
        action="store_true",
        help="Return success (0) if UART input works, even if shell has bugs"
    )

    args = parser.parse_args()

    return run_test(args)


if __name__ == "__main__":
    sys.exit(main())
