#!/usr/bin/env python3
"""Read-only macOS stack samples at window-manager case starts."""
import json
from pathlib import Path
import platform
import re
import signal
import subprocess
import time

# These are the recovery and positive-click cases from the retained measurement.
# Verdict lines close the case window.
CLICK_CASES = frozenset(('stacking/click-raises', 'stacking/all-minimized', 'focus/pair-click'))
START = re.compile(r'^CASE window-manager ([a-z-]+/[a-z-]+) START$')
END = re.compile(r'^(?:CASE window-manager ([a-z-]+/[a-z-]+) END|SUITE window-manager CASE ([a-z-]+/[a-z-]+) (?:PASS|FAIL|SKIP)(?: .*)?)$')
CONTINUOUS_CASES = frozenset(('recovery/stack-restored', 'recovery/supervisor-restarts'))
SAMPLE_SECONDS = 3


class CaseSamples:
    def __init__(self, qemu_pid, serial, system=None, pid_file=None):
        self.launch_pid = qemu_pid
        self.qemu_pid = 0 if pid_file else qemu_pid
        self.pid_file = pid_file
        self.directory = Path(str(serial) + '.qemu-samples')
        self.enabled = (system or platform.system()) == 'Darwin'
        self.pending = ''
        self.seen = set()
        self.children = []
        self.active = set()
        self.windows = {}
        self.window_counts = {}
        if self.enabled:
            self.directory.mkdir(parents=True, exist_ok=True)

    def record(self, event, **fields):
        with (self.directory / 'samples.jsonl').open('a') as output:
            row = dict(event=event, host_unix_ns=time.time_ns(),
                       host_monotonic_ns=time.monotonic_ns(),
                       launch_pid=self.launch_pid, qemu_pid=self.qemu_pid)
            row.update(fields)
            output.write(json.dumps(row) + '\n')

    def observe(self, chunk):
        if not self.enabled:
            return
        self.pending += chunk
        while '\n' in self.pending:
            line, self.pending = self.pending.split('\n', 1)
            line = line.rstrip('\r')
            end = END.fullmatch(line)
            if end:
                case = end[1] or end[2]
                if case in self.active:
                    self.active.discard(case)
                    self.record('sample-case-end', case=case)
                    remaining = []
                    for child, deadline, name in self.children:
                        if name == case:
                            self.finish(child, name)
                        else:
                            remaining.append((child, deadline, name))
                    self.children = remaining
                continue
            match = START.fullmatch(line)
            if not match:
                continue
            case = match[1]
            if case in self.seen or not (case.startswith('recovery/') or case in CLICK_CASES):
                continue
            self.seen.add(case)
            if self.pid_file:
                try:
                    pid = int(self.pid_file.read_text().strip())
                    if pid <= 0:
                        raise ValueError("invalid QEMU pidfile")
                    self.qemu_pid = pid
                except (OSError, ValueError) as error:
                    self.qemu_pid = 0
                    self.record('sample-error', case=case, error=str(error))
                    continue
            if case in CONTINUOUS_CASES:
                self.active.add(case)
                self.record('sample-case-start', case=case)
            self.start_window(case)
        # A malformed/unterminated serial line must not grow the watcher forever.
        self.pending = self.pending[-4096:]

    def start_window(self, case):
        window = self.window_counts.get(case, 0) + 1
        self.window_counts[case] = window
        suffix = f'.{window:04d}' if case in CONTINUOUS_CASES else ''
        target = self.directory / (case.replace('/', '-') + suffix + '.sample.txt')
        start_unix, start_mono = time.time_ns(), time.monotonic_ns()
        try:
            with target.with_suffix('.stderr.txt').open('w') as errors:
                child = subprocess.Popen(['/usr/bin/sample', str(self.qemu_pid),
                                          str(SAMPLE_SECONDS), '1', '-file', str(target)],
                                         stdout=subprocess.DEVNULL, stderr=errors)
        except OSError as error:
            self.record('sample-error', case=case, window=window, error=str(error))
            self.active.discard(case)
            return
        self.windows[child.pid] = (window, start_unix, start_mono)
        self.children.append((child, time.monotonic() + SAMPLE_SECONDS + 1, case))
        self.record('sample-start', case=case, window=window, sample_pid=child.pid, file=str(target),
                    host_unix_ns=start_unix, host_monotonic_ns=start_mono)

    def tick(self, now=None):
        now = time.monotonic() if now is None else now
        remaining, restart = [], []
        for child, deadline, case in self.children:
            if child.poll() is not None or now >= deadline:
                self.finish(child, case)
                if case in self.active:
                    restart.append(case)
            else:
                remaining.append((child, deadline, case))
        self.children = remaining
        for case in restart:
            self.start_window(case)

    def finish(self, child, case):
        # children stores handles returned by this watcher's Popen. Cleanup
        # signals these handles; the QEMU target is only an argument to sample.
        if child.poll() is None:
            # sample handles SIGINT by writing the partial report. SIGTERM
            # discards it, including the final window at a CASE end.
            child.send_signal(signal.SIGINT)
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                child.kill()
        code = child.wait()
        end_unix, end_mono = time.time_ns(), time.monotonic_ns()
        window, start_unix, start_mono = self.windows.pop(child.pid)
        self.record('sample-stop', case=case, window=window, sample_pid=child.pid, returncode=code,
                    host_start_unix_ns=start_unix, host_start_monotonic_ns=start_mono,
                    host_end_unix_ns=end_unix, host_end_monotonic_ns=end_mono)

    def close(self):
        self.active.clear()
        for child, _, case in self.children:
            self.finish(child, case)
        self.children.clear()
