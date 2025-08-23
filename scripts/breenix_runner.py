#!/usr/bin/env python3
"""
Breenix runner with command injection capability
"""

import subprocess
import sys
import os
import time
import socket
import select
import threading
from datetime import datetime
import pty
import termios
import tty
import re

class BreenixRunner:
    def __init__(self, mode="uefi", display=False,
                 enable_ci_ring3_mode=False,
                 timeout_seconds: int = 480,
                 success_any_patterns=None,
                 success_all_patterns=None,
                 failure_patterns=None):
        self.mode = mode
        self.display = display
        self.process = None
        self.master_fd = None
        self.log_file = None
        self.log_path = self._create_log_file()
        # CI ring3 streaming detection configuration
        self.enable_ci_ring3_mode = enable_ci_ring3_mode
        self.timeout_seconds = timeout_seconds
        # Prefer routing guest serial to stdio so CI captures it reliably.
        # If firmware debug is captured to file (BREENIX_QEMU_DEBUGCON_FILE),
        # we can safely keep serial on stdio. Only route to file if explicitly requested.
        self._serial_to_file = os.environ.get("BREENIX_QEMU_SERIAL_TO_FILE") == "1"
        self._serial_log_path = None

        # Default patterns for success/failure detection
        default_success_any = [
            r"\[ OK \] RING3_SMOKE: userspace executed \+ syscall path verified",
            r"🎯 KERNEL_POST_TESTS_COMPLETE 🎯",
        ]
        default_success_all = [
            r"Hello from userspace! Current time:",
            r"Context switch: from_userspace=true, CS=0x33",
        ]
        default_failure = [
            r"DOUBLE FAULT",
            r"Page Fault|PAGE FAULT",
            r"\bpanic\b",
            r"backtrace",
        ]

        self.success_any_patterns = [re.compile(p) for p in (success_any_patterns or default_success_any)]
        self.success_all_patterns = [re.compile(p) for p in (success_all_patterns or default_success_all)]
        self.failure_patterns = [re.compile(p) for p in (failure_patterns or default_failure)]

        # Streaming detection state
        self._success_all_hits = [False] * len(self.success_all_patterns)
        self._success_event = threading.Event()
        self._failure_event = threading.Event()
        
    def _create_log_file(self):
        """Create timestamped log file"""
        script_dir = os.path.dirname(os.path.abspath(__file__))
        project_root = os.path.dirname(script_dir)
        logs_dir = os.path.join(project_root, "logs")
        os.makedirs(logs_dir, exist_ok=True)
        
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        log_path = os.path.join(logs_dir, f"breenix_{timestamp}.log")
        
        print(f"Logging to: {log_path}")
        self.log_file = open(log_path, 'w')
        return log_path
        
    def start(self):
        """Start Breenix with stdio for serial output"""
        # No need for PTY when using stdio
        self.master_fd = None
        slave_fd = None
        
        # Prefer running the built binary directly to avoid grandchild stdio issues in CI
        bin_name = f"qemu-{self.mode}"
        built_bin = os.path.join(os.getcwd(), "target", "release", bin_name)
        if os.path.exists(built_bin):
            cmd = [built_bin]
        else:
            # Fallback to cargo run locally
            cmd = ["cargo", "run", "--release", "--features", "testing", "--bin", bin_name, "--"]
        
        # Add QEMU arguments
        # Route serial appropriately
        if self._serial_to_file:
            # Use a dedicated serial log to avoid colliding with our main log file handle
            self._serial_log_path = self.log_path.replace(
                ".log", "_serial.log"
            )
            cmd.extend(["-serial", f"file:{self._serial_log_path}"])
        else:
            cmd.extend(["-serial", "stdio"])
        if not self.display:
            cmd.extend(["-display", "none"])
            
        print(f"Starting Breenix in {self.mode} mode...")
        
        # Start the process
        self.process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            universal_newlines=True
        )
        
        # Start threads to handle output
        self._start_output_threads()
        if self.enable_ci_ring3_mode and self._serial_to_file and self._serial_log_path:
            self._start_serial_tail_thread()
        
        # Wait for kernel to initialize
        print("Waiting for kernel to initialize...")
        time.sleep(5)
        
    def _start_output_threads(self):
        """Start threads to handle serial and process output"""
        # No need for serial reading when using stdio
        def read_serial():
            pass  # Disabled for stdio mode
                    
        # Thread to read process stdout
        def read_stdout():
            while self.process and self.process.poll() is None:
                line = self.process.stdout.readline()
                if line:
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    # Always write QEMU stdout to the log file (captures QEMU errors)
                    self.log_file.write(line)
                    self.log_file.flush()
                    if self.enable_ci_ring3_mode:
                        self._ingest_line_for_markers(line)
                    
        threading.Thread(target=read_serial, daemon=True).start()
        threading.Thread(target=read_stdout, daemon=True).start()

    def _start_serial_tail_thread(self):
        """Tail the serial log file and mirror content into the main log and marker engine."""
        path = self._serial_log_path
        def tail_file():
            pos = 0
            while self.process and self.process.poll() is None:
                try:
                    with open(path, 'r') as f:
                        f.seek(pos)
                        data = f.read()
                        if data:
                            pos = f.tell()
                            for line in data.splitlines(True):
                                # Mirror to stdout and main log
                                sys.stdout.write(line)
                                sys.stdout.flush()
                                self.log_file.write(line)
                                self.log_file.flush()
                                # Feed marker detector
                                self._ingest_line_for_markers(line)
                except FileNotFoundError:
                    pass
                time.sleep(0.1)
        threading.Thread(target=tail_file, daemon=True).start()
        
    def send_command(self, command):
        """Send a command to the serial console"""
        if not self.master_fd:
            print("Error: Breenix not running")
            return
            
        print(f"\n>>> Sending command: {command}")
        self.log_file.write(f"\n>>> Sending command: {command}\n")
        self.log_file.flush()
        
        # Add newline if not present
        if not command.endswith('\n'):
            command += '\n'
            
        os.write(self.master_fd, command.encode())
        time.sleep(0.5)  # Give a bit of time for the command to be processed
        
    def send_key(self, key):
        """Send a special key combination (e.g., Ctrl+U)"""
        if not self.master_fd:
            print("Error: Breenix not running")
            return
            
        key_map = {
            'ctrl+u': b'\x15',  # Ctrl+U
            'ctrl+p': b'\x10',  # Ctrl+P
            'ctrl+f': b'\x06',  # Ctrl+F
            'ctrl+e': b'\x05',  # Ctrl+E
            'ctrl+t': b'\x14',  # Ctrl+T
            'ctrl+m': b'\x0d',  # Ctrl+M (Enter)
            'ctrl+c': b'\x03',  # Ctrl+C
        }
        
        key_lower = key.lower()
        if key_lower in key_map:
            print(f"\n>>> Sending key: {key}")
            os.write(self.master_fd, key_map[key_lower])
        else:
            print(f"Unknown key combination: {key}")
            
    def wait(self):
        """Wait for the process to complete"""
        if self.process:
            self.process.wait()
            
    def stop(self):
        """Stop Breenix"""
        if self.process:
            print("\nStopping Breenix...")
            self.process.terminate()
            time.sleep(1)
            if self.process.poll() is None:
                self.process.kill()
                
        if self.master_fd:
            os.close(self.master_fd)
            
        if self.log_file:
            self.log_file.close()
            print(f"\nLog saved to: {self.log_path}")

    # CI/Ring3 streaming detection helpers
    def _ingest_line_for_markers(self, line: str) -> None:
        """Analyze a single output line for success/failure markers."""
        # Failure first: fail fast
        for pattern in self.failure_patterns:
            if pattern.search(line):
                self._failure_event.set()
                return

        # Any single success marker
        for pattern in self.success_any_patterns:
            if pattern.search(line):
                self._success_event.set()
                return

        # All-of success markers: update hits and check
        updated = False
        for index, pattern in enumerate(self.success_all_patterns):
            if not self._success_all_hits[index] and pattern.search(line):
                self._success_all_hits[index] = True
                updated = True
        if updated and all(self._success_all_hits):
            self._success_event.set()

    def wait_for_markers_or_exit(self) -> int:
        """Wait until success or failure markers are observed, process exits, or timeout.

        Returns process exit code (0 for success if markers observed, 1 on failure/timeout).
        """
        start = time.monotonic()
        while True:
            # Marker-based exit
            if self._failure_event.is_set():
                print("\n[CI] Detected failure marker in QEMU output. Terminating...")
                self.stop()
                return 1
            if self._success_event.is_set():
                print("\n[CI] Detected ring3 success markers in QEMU output. Terminating...")
                self.stop()
                return 0

            # Process exit
            if self.process and self.process.poll() is not None:
                code = self.process.returncode
                print(f"\n[CI] QEMU process exited with code {code}.")
                # QEMU isa-debug-exit encodes exit as (value << 1) | 1
                # We treat codes 0x21 (0x10<<1|1) as success, 0x23 (0x11<<1|1) as failure
                if code == 0x21:
                    return 0
                if code == 0x23:
                    return 1
                return 0 if code == 0 else 1

            # Timeout
            if (time.monotonic() - start) > self.timeout_seconds:
                print("\n[CI] Timeout waiting for ring3 markers. Terminating...")
                self.stop()
                return 1

            time.sleep(0.1)
            
def main():
    """Run Breenix with optional commands"""
    import argparse
    
    parser = argparse.ArgumentParser(description='Run Breenix with command injection')
    parser.add_argument('--mode', choices=['uefi', 'bios'], default='uefi',
                        help='Boot mode (default: uefi)')
    parser.add_argument('--display', action='store_true',
                        help='Show QEMU display window')
    parser.add_argument('--commands', nargs='*',
                        help='Commands to send after boot')
    parser.add_argument('--interactive', action='store_true',
                        help='Stay in interactive mode after commands')
    # CI/Ring3 streaming detection
    parser.add_argument('--ci-ring3', action='store_true',
                        help='Enable CI ring3 mode: stream QEMU output and exit early on success/failure markers')
    parser.add_argument('--timeout-seconds', type=int, default=480,
                        help='Timeout for CI ring3 mode (default: 480 seconds)')
    parser.add_argument('--success-any', action='append', default=None,
                        help='Regex for success-any pattern (can be repeated)')
    parser.add_argument('--success-all', action='append', default=None,
                        help='Regex for success-all pattern (all must be seen; can be repeated)')
    parser.add_argument('--failure', action='append', default=None,
                        help='Regex for failure pattern (can be repeated)')
    
    args = parser.parse_args()
    
    runner = BreenixRunner(
        mode=args.mode,
        display=args.display,
        enable_ci_ring3_mode=args.ci_ring3,
        timeout_seconds=args.timeout_seconds,
        success_any_patterns=args.success_any,
        success_all_patterns=args.success_all,
        failure_patterns=args.failure,
    )
    
    try:
        runner.start()
        
        # Send any commands
        if args.commands:
            for cmd in args.commands:
                runner.send_command(cmd)
                time.sleep(1)  # Give command time to execute
                
        # Interactive mode or wait
        if args.interactive:
            print("\nEntering interactive mode. Type 'exit' to quit.")
            print("Special keys: ctrl+u, ctrl+p, ctrl+f, ctrl+e, ctrl+t, ctrl+m")
            
            while True:
                try:
                    user_input = input("> ").strip()
                    if user_input.lower() == 'exit':
                        break
                    elif user_input.lower().startswith('ctrl+'):
                        runner.send_key(user_input)
                    else:
                        runner.send_command(user_input)
                except KeyboardInterrupt:
                    break
        else:
            # CI ring3 mode: watch output and exit early; otherwise wait for process
            if args.ci_ring3:
                exit_code = runner.wait_for_markers_or_exit()
                # Ensure log file is closed and QEMU stopped
                runner.stop()
                sys.exit(exit_code)
            else:
                runner.wait()
            
    except KeyboardInterrupt:
        print("\nInterrupted by user")
    finally:
        runner.stop()
        
if __name__ == '__main__':
    main()