#!/usr/bin/env python3
"""Host receipt clocks for complete serial lines, with original byte offsets."""
import json
import time

class SerialClock:
    def __init__(self, serial):
        self.serial = serial
        self.path = serial.with_name(serial.name + '.host-clock.jsonl')
        self.offset = 0
        self.pending = b''
        self.line_offset = 0
        self.path.write_text('')

    def observe(self, unix_ns=None, monotonic_ns=None):
        unix_ns = time.time_ns() if unix_ns is None else unix_ns
        monotonic_ns = time.monotonic_ns() if monotonic_ns is None else monotonic_ns
        try:
            with self.serial.open('rb') as source:
                if self.serial.stat().st_size < self.offset:
                    self.offset = self.line_offset = 0
                    self.pending = b''
                source.seek(self.offset)
                data = source.read()
        except FileNotFoundError:
            return
        self.offset += len(data)
        self.pending += data
        lines = self.pending.split(b'\n')
        self.pending = lines.pop()
        with self.path.open('a') as target:
            for line in lines:
                target.write(json.dumps({'host_unix_ns': unix_ns,
                    'host_monotonic_ns': monotonic_ns, 'offset': self.line_offset,
                    'line': line.decode('utf-8', errors='replace')}) + '\n')
                self.line_offset += len(line) + 1
