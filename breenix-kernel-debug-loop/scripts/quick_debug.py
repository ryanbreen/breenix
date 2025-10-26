#!/usr/bin/env python3
"""
Fast kernel debug loop with signal detection.

Runs the Breenix kernel for up to a specified timeout (default 15s),
monitoring logs in real-time for specific signals. Terminates immediately
when the signal is found or when the timeout expires.
"""

import argparse
import os
import subprocess
import sys
import time
import signal as sig
from pathlib import Path
from datetime import datetime
import select


class DebugSession:
    def __init__(self, signal_pattern=None, timeout=15, mode="uefi", quiet=False):
        self.signal_pattern = signal_pattern
        self.timeout = timeout
        self.mode = mode
        self.quiet = quiet
        self.process = None
        self.output_buffer = []
        self.signal_found = False
        self.start_time = None

    def run(self):
        """Execute the debug session."""
        project_root = Path(__file__).parent.parent.parent.resolve()

        # Determine the cargo command
        if self.mode == "bios":
            cmd = ["cargo", "run", "--release", "--features", "testing",
                   "--bin", "qemu-bios", "--", "-serial", "stdio", "-display", "none"]
        else:
            cmd = ["cargo", "run", "--release", "--features", "testing",
                   "--bin", "qemu-uefi", "--", "-serial", "stdio", "-display", "none"]

        if not self.quiet:
            print(f"🔍 Starting kernel debug session ({self.timeout}s timeout)", file=sys.stderr)
            if self.signal_pattern:
                print(f"   Watching for signal: {self.signal_pattern}", file=sys.stderr)
            print("", file=sys.stderr)

        self.start_time = time.time()

        # Start the process
        self.process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            cwd=project_root,
            text=True,
            bufsize=1,  # Line buffered
        )

        # Set up signal handler for clean termination
        sig.signal(sig.SIGINT, self._signal_handler)
        sig.signal(sig.SIGTERM, self._signal_handler)

        try:
            self._monitor_output()
        finally:
            self._cleanup()

        return self._generate_report()

    def _monitor_output(self):
        """Monitor process output in real-time."""
        while True:
            # Check timeout
            elapsed = time.time() - self.start_time
            if elapsed >= self.timeout:
                if not self.quiet:
                    print(f"\n⏱️  Timeout reached ({self.timeout}s)", file=sys.stderr)
                break

            # Check if process is still running
            if self.process.poll() is not None:
                # Process terminated, read any remaining output
                remaining = self.process.stdout.read()
                if remaining:
                    for line in remaining.splitlines():
                        self._process_line(line)
                break

            # Read line with timeout
            line = self.process.stdout.readline()
            if line:
                line = line.rstrip('\n')
                self._process_line(line)

                # Check for signal
                if self.signal_pattern and self.signal_pattern in line:
                    self.signal_found = True
                    if not self.quiet:
                        print(f"\n✅ Signal found: {self.signal_pattern}", file=sys.stderr)
                    break
            else:
                # Small sleep to prevent busy waiting
                time.sleep(0.01)

    def _process_line(self, line):
        """Process a single line of output."""
        self.output_buffer.append(line)
        if not self.quiet:
            print(line)

    def _cleanup(self):
        """Clean up the subprocess."""
        if self.process and self.process.poll() is None:
            if not self.quiet:
                print("\n🛑 Terminating kernel...", file=sys.stderr)

            # Try graceful termination first
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                # Force kill if needed
                self.process.kill()
                self.process.wait()

    def _signal_handler(self, signum, frame):
        """Handle interrupt signals."""
        if not self.quiet:
            print("\n\n⚠️  Interrupted by user", file=sys.stderr)
        self._cleanup()
        sys.exit(1)

    def _generate_report(self):
        """Generate a debug report from the session."""
        elapsed = time.time() - self.start_time

        report = {
            'success': self.signal_found if self.signal_pattern else True,
            'signal_found': self.signal_found,
            'signal_pattern': self.signal_pattern,
            'elapsed_time': elapsed,
            'timeout': self.timeout,
            'output_lines': len(self.output_buffer),
            'output': '\n'.join(self.output_buffer),
        }

        if not self.quiet:
            print("\n" + "="*60, file=sys.stderr)
            print("📊 Debug Session Summary", file=sys.stderr)
            print("="*60, file=sys.stderr)
            print(f"Status: {'✅ SUCCESS' if report['success'] else '❌ TIMEOUT'}", file=sys.stderr)
            if self.signal_pattern:
                print(f"Signal: {'Found' if self.signal_found else 'Not found'}", file=sys.stderr)
            print(f"Time: {elapsed:.2f}s / {self.timeout}s", file=sys.stderr)
            print(f"Output lines: {len(self.output_buffer)}", file=sys.stderr)
            print("="*60, file=sys.stderr)

        return report


def main():
    parser = argparse.ArgumentParser(
        description='Fast kernel debug loop with signal detection'
    )
    parser.add_argument(
        '--signal',
        help='Signal pattern to watch for in kernel output'
    )
    parser.add_argument(
        '--timeout',
        type=float,
        default=15.0,
        help='Maximum time to run (seconds, default: 15)'
    )
    parser.add_argument(
        '--mode',
        choices=['uefi', 'bios'],
        default='uefi',
        help='Boot mode (default: uefi)'
    )
    parser.add_argument(
        '--quiet',
        action='store_true',
        help='Suppress progress output, only show kernel output'
    )

    args = parser.parse_args()

    session = DebugSession(
        signal_pattern=args.signal,
        timeout=args.timeout,
        mode=args.mode,
        quiet=args.quiet
    )

    report = session.run()

    # Exit with appropriate code
    sys.exit(0 if report['success'] else 1)


if __name__ == '__main__':
    main()
