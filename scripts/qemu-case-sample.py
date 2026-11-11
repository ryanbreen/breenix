#!/usr/bin/env python3
"""Read-only macOS stack samples at window-manager case starts."""
import json
from pathlib import Path
import platform
import re
import subprocess
import time

# These are the recovery and positive-click cases from the retained measurement.
# Verdict lines are intentionally not triggers: their work has already ended.
CLICK_CASES = frozenset(('stacking/click-raises', 'stacking/all-minimized', 'focus/pair-click'))
START = re.compile(r'^CASE window-manager ([a-z-]+/[a-z-]+) START$')
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
        if self.enabled:
            self.directory.mkdir(parents=True, exist_ok=True)

    def record(self, event, **fields):
        with (self.directory / 'samples.jsonl').open('a') as output:
            output.write(json.dumps(dict(event=event, host_unix_ns=time.time_ns(),
                                         host_monotonic_ns=time.monotonic_ns(),
                                         launch_pid=self.launch_pid, qemu_pid=self.qemu_pid, **fields)) + '\n')

    def observe(self, chunk):
        if not self.enabled:
            return
        self.pending += chunk
        while '\n' in self.pending:
            line, self.pending = self.pending.split('\n', 1)
            match = START.fullmatch(line.rstrip('\r'))
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
            target = self.directory / (case.replace('/', '-') + '.sample.txt')
            try:
                with target.with_suffix('.stderr.txt').open('w') as errors:
                    child = subprocess.Popen(['/usr/bin/sample', str(self.qemu_pid),
                                              str(SAMPLE_SECONDS), '1', '-file', str(target)],
                                             stdout=subprocess.DEVNULL, stderr=errors)
            except OSError as error:
                self.record('sample-error', case=case, error=str(error))
                continue
            self.children.append((child, time.monotonic() + SAMPLE_SECONDS + 1, case))
            self.record('sample-start', case=case, sample_pid=child.pid, file=str(target))
        # A malformed/unterminated serial line must not grow the watcher forever.
        self.pending = self.pending[-4096:]

    def tick(self, now=None):
        now = time.monotonic() if now is None else now
        remaining = []
        for child, deadline, case in self.children:
            if child.poll() is not None or now >= deadline:
                self.finish(child, case)
            else:
                remaining.append((child, deadline, case))
        self.children = remaining

    def finish(self, child, case):
        # children stores handles returned by this watcher's Popen. Cleanup
        # signals these handles; the QEMU target is only an argument to sample.
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                child.kill()
        code = child.wait()
        self.record('sample-stop', case=case, sample_pid=child.pid, returncode=code)

    def close(self):
        for child, _, case in self.children:
            self.finish(child, case)
        self.children.clear()
