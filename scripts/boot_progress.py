"""A boot advances on complete userspace lines or first-seen boot stages.

Kernel diagnostics, census traffic and loader announcements are never progress.
The 120-second inactivity deadline is independent of a launcher's hard deadline.
Records live beside the serial, which is never modified or deleted here.
"""
import datetime
import json
from pathlib import Path
import re
import time

ROOT = Path(__file__).resolve().parents[1]
STALL_SECONDS = 120
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
KERNEL = re.compile(r"^(?:\[\s*(?:INFO|WARN|ERROR|DEBUG|TRACE)\]|\[(?:clock|boot|test|smp)\]|.*\bkernel(?:::|:))")
DIAGNOSTIC = re.compile(
    r"^(?:T\d+)?(?:\[[^]\n]*(?:CENSUS|diagnostic|BXCAP|FATAL_REGS|TIMER_SCALE_ORACLE)"
    r"|\[smp\] user-thread dispatches|(?:BXCAP|FATAL_REGS)\b"
    r"|BP (?:handler|from_userspace)|BP_HANDLER_ENTRY|(?:About to call|Called) preempt_disable"
    r"|breenix>\s*$)", re.I)
ANNOUNCEMENT = re.compile(r"(?:kernel::test_exec:|Userspace will emit|will (?:print|emit)|expected marker|Missing marker|checkHint)", re.I)
USER = re.compile(r"^(?:SUITE |SUITE_SEQUENCE |PROBE |RUN \S+ |TEST_TALLY:|[A-Z][A-Z0-9_]*(?:_TEST|_PASSED|_START|_COMPLETE)\b|\[(?:[a-z][a-z0-9_]*|(?i:parent|child))\])")
DONE = re.compile(r"^(?:SUITE \S+ DONE passed=\d+ failed=\d+ skipped=\d+ total=\d+|PROBE DONE passed=\d+ failed=\d+|USERSPACE TEST REPORT DONE|RUN \S+ DONE .+)$")


def stamp(epoch):
    return datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).isoformat()


class BootProgress:
    def __init__(self, serial, *, now=None, epoch=None, timeout=STALL_SECONDS, track_completion=False):
        self.track_completion = track_completion
        self.serial = Path(serial)
        self.timeout = timeout
        self.last_at = time.monotonic() if now is None else now
        self.last_epoch = time.time() if epoch is None else epoch
        self.last_line = "<no progress line>"
        self.last_source = None
        self.last_user_line = None
        self.last_user_source = None
        self.last_user_epoch = None
        self.stages = set()
        self.streams = {}
        self.file_streams = {}
        self.completed = False
        self.recorded = None
        catalogs = ROOT / "tools/breenix-runs/Sources/BreenixRuns/Resources"
        self.markers = {marker for arch in ("aarch64", "x86_64")
                        for stage in json.loads((catalogs / f"boot-stages-{arch}.json").read_text())["stages"]
                        for marker in stage["marker"].split("|")}
        for milestone in json.loads((ROOT / "docs/boot-path.json").read_text())["milestones"]:
            for stages in milestone["stages"].values():
                self.markers.update(marker for stage in stages if isinstance(stage, dict)
                                    for marker in stage["marker"].split("|"))
        self.longer_markers = {marker: tuple(other for other in self.markers
                                              if len(other) > len(marker) and marker in other)
                               for marker in self.markers}

    def line(self, line, source, *, userspace=False, now=None, epoch=None):
        line = ANSI.sub("", line).strip("\r ")
        if not line or ANNOUNCEMENT.search(line) or DIAGNOSTIC.search(line):
            return
        kernel = bool(KERNEL.search(line)) or (
            not userspace and Path(source).name in
            ("serial_kernel.log", "serial-kernel.txt", "serial_kernel.txt"))
        # Accept producer-tagged userspace records on either UART. Unlabelled
        # output has no trustworthy producer, even on COM1 (kernel debug uses it).
        user = not kernel and bool(USER.match(line))
        stages = set()
        for marker in self.markers - self.stages:
            if marker not in line:
                continue
            masked = line
            for longer in self.longer_markers[marker]:
                masked = masked.replace(longer, "\0")
            if marker in masked:
                stages.add(marker)
        payload = line.split("kernel::syscall::handlers: ", 1)[-1]
        terminal = bool(DONE.fullmatch(payload))
        if kernel and not stages and not terminal:
            return
        if not user and not stages and not terminal:
            return
        self.stages.update(stages)
        self.last_line, self.last_source = line, str(source)
        self.last_at = time.monotonic() if now is None else now
        self.last_epoch = time.time() if epoch is None else epoch
        if user:
            self.last_user_line, self.last_user_source = line, str(source)
            self.last_user_epoch = self.last_epoch
        if user and re.match(r"^(?:SUITE \S+ START |PROBE (?!DONE)|RUN \S+ START)", line):
            self.completed = False
        if self.track_completion:
            if terminal or line == "[init] Boot script completed":
                self.completed = True

    def observe(self, text, source=None, *, userspace=False, now=None, epoch=None):
        """Consume only newly appended, newline-terminated records from a snapshot."""
        source = str(source or self.serial)
        offset = self.streams.get(source, 0)
        if len(text) < offset:
            offset = 0
        end = text.rfind("\n") + 1
        for line in text[offset:end].split("\n")[:-1]:
            self.line(line, source, userspace=userspace, now=now, epoch=epoch)
        self.streams[source] = end

    def poll(self, sources, *, now=None, epoch=None):
        for source, userspace in sources:
            path = Path(source)
            key = str(path)
            offset, partial = self.file_streams.get(key, (0, b""))
            try:
                with path.open("rb") as stream:
                    if stream.seek(0, 2) < offset:
                        offset, partial = 0, b""
                    stream.seek(offset)
                    data = stream.read()
            except FileNotFoundError:
                continue
            lines = (partial + data).split(b"\n")
            self.file_streams[key] = (offset + len(data), lines.pop())
            for line in lines:
                self.line(line.decode(errors="replace"), path, userspace=userspace,
                          now=now, epoch=epoch)

    def stalled(self, *, now=None):
        now = time.monotonic() if now is None else now
        return not self.completed and now - self.last_at >= self.timeout

    def record(self, *, now=None, epoch=None):
        if self.recorded is not None:
            return self.recorded
        now = time.monotonic() if now is None else now
        epoch = time.time() if epoch is None else epoch
        self.recorded = dict(status="stalled", stopped_at=stamp(epoch), stopped_epoch=epoch,
                             last_progress_line=self.last_line, last_progress_source=self.last_source,
                             last_progress_at=stamp(self.last_epoch),
                             last_userspace_progress_line=self.last_user_line,
                             last_userspace_progress_source=self.last_user_source,
                             last_userspace_progress_at=stamp(self.last_user_epoch) if self.last_user_epoch is not None else None,
                             inactivity_seconds=now - self.last_at, limit_seconds=self.timeout)
        sidecar = Path(str(self.serial) + ".stall.json")
        sidecar.parent.mkdir(parents=True, exist_ok=True)
        import os
        temporary = sidecar.with_suffix(f".{os.getpid()}.tmp")
        temporary.write_text(json.dumps(self.recorded) + "\n")
        temporary.replace(sidecar)
        print(f"STALLED: stopped_at={self.recorded['stopped_at']} "
              f"no test progress for {now - self.last_at:.1f}s; last progress: {self.last_line}", flush=True)
        return self.recorded
