#!/usr/bin/env python3
"""Run the opt-in breenfs validation battery and record it for Vigil.

The battery runs every expensive breenfs host check in one go: the crash and
recovery corpus, the VFS model and conformance tests, the full-disk fill and
delete stream, and seeded breenfs-torture. It is never a per-change gate. It
starts only when the operator asks (Vigil's Validation tab or his own shell).

    python3 scripts/breenfs-battery.py --profile quick|full \
        [--torture-seeds N] [--stages host-corpus,vfs,fill-delete,torture] [--out DIR]

The run folder layout, event lines and outcomes follow
fx/briefs/breenfs-battery-contract.md. Standard library only.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import fcntl
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Optional

REPO = Path(__file__).resolve().parent.parent
DEFAULT_ROOT = Path.home() / "Library" / "Application Support" / "Vigil" / "breenix" / "batteries"
EXIT_LOCKED = 75
EXIT_STOPPED = 143
STOP_GRACE_SECONDS = 30.0
TORTURE_PROCESSES = 4
TORTURE_PROGRESS_SECONDS = 10.0
MESSAGE_LIMIT = 500
FILL_DELETE_TEST = "fill_and_delete_seeds_1_to_16_match_the_model"
FILL_DELETE_SEEDS = 16
STAGE_ORDER = ("host-corpus", "vfs", "fill-delete", "torture")
PROFILES = {"full": STAGE_ORDER, "quick": STAGE_ORDER[:3]}


@dataclass(frozen=True)
class KnownIssue:
    """A failure that is main's known issue: the test, and what its failure must say."""

    test: str
    issue: str
    message: re.Pattern

    def matches(self, name: str, text: str) -> bool:
        named = name == self.test or name.endswith("::" + self.test)
        return named and self.message.search(text) is not None


# v1: the crash corpus's wall-clock bound (#3023). Only a failure of that bound
# is known; any other failure of these tests is a real failure.
ELAPSED_BOUND = re.compile(r"exceeded 10 minutes|elapsed\(\)\.as_secs_f64\(\) < 600")
KNOWN_ISSUES = (
    KnownIssue("torn_images_recover_to_an_acknowledged_commit", "#3023", ELAPSED_BOUND),
    KnownIssue("subset_images_recover_to_an_acknowledged_commit", "#3023", ELAPSED_BOUND),
)


def now_iso() -> str:
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_json_atomic(path: Path, value: object) -> None:
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def command_prefix() -> list[str]:
    """Every command runs at low priority: nice 15 and, on macOS, background QoS."""
    prefix = ["nice", "-n", "15"]
    if shutil.which("taskpolicy"):
        prefix += ["taskpolicy", "-b"]
    return prefix


def command_env() -> dict[str, str]:
    env = dict(os.environ)
    env["CARGO_BUILD_JOBS"] = "4"
    # Cargo targets live in the checkout's own target directories.
    env.pop("CARGO_TARGET_DIR", None)
    return env


@dataclass
class Finish:
    outcome: str
    done: int
    failures: int
    summary: str


class Stopped(Exception):
    """The operator stopped the run while a stage was working."""


# --------------------------------------------------------------------------
# libtest output


@dataclass
class Case:
    name: str
    outcome: str  # pass | fail | known | ignored
    seconds: Optional[float] = None
    message: str = ""
    issue: str = ""


RESULT_LINE = re.compile(r"^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED|ignored)(.*)$")
START_LINE = re.compile(r"^test (\S+)(?: - should panic)? \.\.\. $")
BARE_RESULT = re.compile(r"^(ok|FAILED|ignored)(.*)$")
SECTION_LINE = re.compile(r"^---- (\S+) stdout ----$")
REPORT_TIME = re.compile(r"<([0-9]+(?:\.[0-9]+)?)s>\s*$")
OLD_PANIC = re.compile(r"panicked at '(.*)', ")


def failure_message(text: str) -> str:
    """The first line of the panic message in a libtest failure section."""
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if "panicked at" not in line:
            continue
        old = OLD_PANIC.search(line)
        if old:
            return old.group(1)[:MESSAGE_LIMIT]
        for after in lines[index + 1:]:
            if after.strip():
                return after.strip()[:MESSAGE_LIMIT]
    for line in lines:
        if line.strip():
            return line.strip()[:MESSAGE_LIMIT]
    return ""


class LibtestParser:
    """Turn libtest's pretty output into case events.

    Passed and ignored cases are reported as their result lines arrive. A failed
    case waits for its binary's failures section, which carries the message that
    decides whether it is a known issue, and is reported at `test result:`.
    """

    def __init__(self, report: Callable[[Case], None], known: tuple[KnownIssue, ...]):
        self.report = report
        self.known = known
        self.cases: list[Case] = []
        self._started: Optional[str] = None
        self._failed: list[Case] = []
        self._sections: dict[str, list[str]] = {}
        self._section: Optional[str] = None

    def feed(self, line: str) -> None:
        if self._section is not None:
            if SECTION_LINE.match(line) or line == "failures:" or line.startswith("test result:"):
                self._section = None
            else:
                self._sections[self._section].append(line)
                return
        section = SECTION_LINE.match(line)
        if section:
            self._section = section.group(1)
            self._sections[self._section] = []
            return
        if line.startswith("test result:"):
            self.flush()
            return
        result = RESULT_LINE.match(line)
        if result:
            self._started = None
            self._result(result.group(1), result.group(2), result.group(3))
            return
        start = START_LINE.match(line)
        if start:
            self._started = start.group(1)
            return
        bare = BARE_RESULT.match(line)
        if bare and self._started:
            name, self._started = self._started, None
            self._result(name, bare.group(1), bare.group(2))

    def _result(self, name: str, result: str, rest: str) -> None:
        seconds = None
        if result != "ignored":
            timed = REPORT_TIME.search(rest)
            if timed:
                seconds = float(timed.group(1))
        if result == "FAILED":
            self._failed.append(Case(name, "fail", seconds))
            return
        self._emit(Case(name, "pass" if result == "ok" else "ignored", seconds))

    def _emit(self, case: Case) -> None:
        self.cases.append(case)
        self.report(case)

    def flush(self) -> None:
        """Report failed cases with their messages (also at the end of a stage)."""
        failed, self._failed = self._failed, []
        for case in failed:
            text = "\n".join(self._sections.pop(case.name, []))
            case.message = failure_message(text)
            for issue in self.known:
                if issue.matches(case.name, text):
                    case.outcome = "known"
                    case.issue = issue.issue
                    break
            self._emit(case)


def list_tests(output: str) -> list[str]:
    return [line[: -len(": test")] for line in output.splitlines() if line.endswith(": test")]


def case_summary(cases: list[Case]) -> str:
    counts = {k: sum(1 for c in cases if c.outcome == k) for k in ("pass", "fail", "known", "ignored")}
    parts = [f"{counts['pass']} passed"]
    if counts["fail"]:
        parts.append(f"{counts['fail']} failed")
    if counts["known"]:
        issues = sorted({c.issue for c in cases if c.outcome == "known"})
        parts.append(f"{counts['known']} known ({', '.join(issues)})")
    if counts["ignored"]:
        parts.append(f"{counts['ignored']} ignored")
    return ", ".join(parts)


# --------------------------------------------------------------------------
# Stages


@dataclass
class Stage:
    id: str
    title: str
    checks: str
    command: str
    unit: str
    planned: Optional[int]
    known_issues: tuple[KnownIssue, ...] = ()

    def describe(self) -> dict:
        return {
            "id": self.id,
            "title": self.title,
            "checks": self.checks,
            "command": self.command,
            "unit": self.unit,
            "planned": self.planned,
            "knownIssues": [{"test": k.test, "issue": k.issue} for k in self.known_issues],
        }

    def run(self, battery: "Battery") -> Finish:
        raise NotImplementedError


@dataclass
class LibtestStage(Stage):
    """A libtest run: the list command builds the tests and counts the planned ones."""

    list_argv: list[str] = field(default_factory=list)
    run_argv: list[str] = field(default_factory=list)
    cwd: Path = REPO

    def run(self, battery: "Battery") -> Finish:
        listing = battery.capture(self, self.list_argv, self.cwd)
        if listing.code != 0:
            battery.stage_start(self, None)
            return Finish("fail", 0, 0, f"build or test listing failed (exit {listing.code})")
        planned = len(list_tests(listing.output))
        battery.stage_start(self, planned)
        parser = LibtestParser(lambda case: battery.case(self, case), self.known_issues)
        try:
            code = battery.stream(self, self.run_argv, self.cwd, parser.feed)
        except Stopped:
            parser.flush()
            return stopped_finish(parser.cases)
        parser.flush()
        return test_finish(parser.cases, planned, code)


def stopped_finish(cases: list[Case]) -> Finish:
    failures = sum(1 for c in cases if c.outcome in ("fail", "known"))
    return Finish("stopped", len(cases), failures, f"stopped by the operator after {case_summary(cases)}")


def test_finish(cases: list[Case], planned: int, code: int) -> Finish:
    failed = sum(1 for c in cases if c.outcome == "fail")
    known = sum(1 for c in cases if c.outcome == "known")
    summary = case_summary(cases)
    complete = len(cases) == planned
    if failed:
        outcome = "fail"
    elif not complete:
        outcome = "fail"
        summary += f"; {len(cases)} of {planned} tests reported (exit {code})"
    elif code != 0 and not known:
        outcome = "fail"
        summary += f"; exit {code}"
    elif known:
        outcome = "known"
    else:
        outcome = "pass"
    return Finish(outcome, len(cases), failed + known, summary)


FILL_SEED = re.compile(r"fill/delete seed (\d+): PASS")
FAILED_SEED = re.compile(r"seed=(\d+)")


@dataclass
class FillDeleteStage(Stage):
    """The ignored fill/delete stream, by name; one progress event per finished seed."""

    list_argv: list[str] = field(default_factory=list)
    run_argv: list[str] = field(default_factory=list)  # the exact test name is appended
    cwd: Path = REPO
    seeds: int = FILL_DELETE_SEEDS

    def run(self, battery: "Battery") -> Finish:
        listing = battery.capture(self, self.list_argv, self.cwd)
        names = [n for n in list_tests(listing.output) if n == FILL_DELETE_TEST or n.endswith("::" + FILL_DELETE_TEST)]
        battery.stage_start(self, self.seeds)
        if listing.code != 0:
            return Finish("fail", 0, 0, f"build or test listing failed (exit {listing.code})")
        if len(names) != 1:
            return Finish("fail", 0, 0, f"{FILL_DELETE_TEST} is not in tools/vfs-host-tests")
        passed: set[int] = set()

        def progress(state: dict[int, str]) -> None:
            seeds = [{"seed": s, "state": state.get(s, "waiting")} for s in range(1, self.seeds + 1)]
            failures = sum(1 for s in seeds if s["state"] == "fail")
            battery.event("progress", stage=self.id, done=len(passed), planned=self.seeds,
                          failures=failures, seeds=seeds)

        parser = LibtestParser(lambda case: battery.case(self, case), self.known_issues)

        def feed(line: str) -> None:
            seed = FILL_SEED.search(line)
            if seed:
                passed.add(int(seed.group(1)))
                progress({s: "pass" for s in passed})
                return
            parser.feed(line)

        try:
            code = battery.stream(self, self.run_argv + [names[0]], self.cwd, feed)
        except Stopped:
            parser.flush()
            return Finish("stopped", len(passed), 0,
                          f"stopped by the operator after {len(passed)} of {self.seeds} seeds passed")
        parser.flush()
        failed = [c for c in parser.cases if c.outcome == "fail"]
        bad: set[int] = set()
        for case in failed:
            seed = FAILED_SEED.search(case.message)
            if seed:
                bad.add(int(seed.group(1)))
        if failed or code != 0 or len(passed) != self.seeds:
            state = {s: "pass" for s in passed}
            state.update({s: "fail" for s in bad})
            for s in range(1, self.seeds + 1):
                state.setdefault(s, "waiting")
            progress(state)
            detail = failed[0].message if failed else f"exit {code}"
            return Finish("fail", len(passed), max(len(bad), 1),
                          f"{len(passed)} of {self.seeds} seeds passed; {detail}"[:MESSAGE_LIMIT])
        return Finish("pass", len(passed), 0, f"{len(passed)} seeds passed")


@dataclass
class TortureStage(Stage):
    """Seeded breenfs-torture in release builds, one process per seed, at most four at once."""

    seeds: int = 8
    builds: list[tuple[list[str], Path]] = field(default_factory=list)
    torture: Callable[[], Path] = lambda: Path()
    checker: Callable[[], Path] = lambda: Path()
    processes: int = TORTURE_PROCESSES

    def run(self, battery: "Battery") -> Finish:
        battery.stage_start(self, self.seeds)
        for argv, cwd in self.builds:
            built = battery.capture(self, argv, cwd)
            if built.code != 0:
                return Finish("fail", 0, 0, f"release build failed (exit {built.code})")
        torture, checker = self.torture(), self.checker()
        out = battery.run_dir / "stages" / "torture"
        out.mkdir(parents=True, exist_ok=True)
        state = {s: {"seed": s, "state": "waiting", "images": 0, "failures": 0} for s in range(1, self.seeds + 1)}
        waiting = list(range(1, self.seeds + 1))
        running: dict[int, subprocess.Popen] = {}
        last = {"at": 0.0, "key": None}

        def read_progress(seed: int) -> None:
            path = out / f"seed-{seed}" / "progress.json"
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                return
            for entry in data.get("seeds", []):
                if entry.get("seed") == seed:
                    state[seed]["images"] = int(entry.get("images", 0))
                    state[seed]["failures"] = int(entry.get("failures", 0))

        def emit(force: bool = False) -> None:
            key = tuple((s["state"], s["images"]) for s in state.values())
            moved = key != last["key"]
            if not force and (not moved or time.monotonic() - last["at"] < TORTURE_PROGRESS_SECONDS):
                return
            last["at"], last["key"] = time.monotonic(), key
            active = [s for s in state.values() if s["state"] == "running"]
            current = ", ".join(str(s["seed"]) for s in active)
            battery.event(
                "progress", stage=self.id,
                done=sum(1 for s in state.values() if s["state"] in ("pass", "fail")),
                planned=self.seeds,
                failures=sum(1 for s in state.values() if s["state"] == "fail"),
                images=sum(s["images"] for s in state.values()),
                current=(f"seed {current}" if len(active) == 1 else f"seeds {current}") if active else "",
                seeds=[{"seed": s["seed"], "state": s["state"], "images": s["images"]} for s in state.values()],
            )

        while waiting or running:
            while waiting and len(running) < self.processes and not battery.stopping:
                seed = waiting.pop(0)
                seed_out = out / f"seed-{seed}"
                seed_out.mkdir(parents=True, exist_ok=True)
                argv = [str(torture), "--seeds", "1", "--seed-start", str(seed), "--checker", str(checker),
                        "--out", str(seed_out), "--progress", str(seed_out / "progress.json")]
                running[seed] = battery.spawn(self, argv, REPO, prefix=f"[seed {seed}] ")
                state[seed]["state"] = "running"
            if battery.stopping and not running:
                break
            battery.supervise(list(running.values()))
            for seed, proc in list(running.items()):
                read_progress(seed)
                if proc.poll() is not None:
                    battery.join_output(proc)
                    del running[seed]
                    state[seed]["state"] = "pass" if proc.returncode == 0 else "fail"
                    if battery.stopping and proc.returncode != 0:
                        state[seed]["state"] = "waiting"
            emit()
            if running:
                time.sleep(0.5)
        emit(force=True)
        images = sum(s["images"] for s in state.values())
        passed = sum(1 for s in state.values() if s["state"] == "pass")
        failed = sum(1 for s in state.values() if s["state"] == "fail")
        summary = f"{passed} of {self.seeds} seeds passed, {images:,} images checked"
        if battery.stopping:
            return Finish("stopped", passed + failed, failed, f"stopped by the operator after {summary}")
        return Finish("fail" if failed or passed != self.seeds else "pass", passed + failed, failed, summary)


def cargo_target(manifest: Path) -> Path:
    output = subprocess.run(
        command_prefix() + ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(manifest)],
        cwd=REPO, env=command_env(), capture_output=True, text=True, check=True,
    ).stdout
    return Path(json.loads(output)["target_directory"])


def breenfs_stages(torture_seeds: int) -> list[Stage]:
    host = REPO / "tools" / "breenfs-host-tests" / "Cargo.toml"
    vfs = REPO / "tools" / "vfs-host-tests" / "Cargo.toml"
    core = REPO / "tools" / "breenfs" / "Cargo.toml"
    report_time = ["-Zunstable-options", "--report-time"]

    def cargo_test(manifest: Path, *libtest: str) -> list[str]:
        return ["cargo", "test", "--no-fail-fast", "--manifest-path", str(manifest), "--", *libtest]

    skip_fill = ["--skip", FILL_DELETE_TEST]
    return [
        LibtestStage(
            id="host-corpus", title="Crash and recovery corpus",
            checks=("Every enumerated crash image (flush-boundary subsets, torn final sectors, single-bit flips and "
                    "the minimized seed regressions) recovers to an acknowledged commit that breenfs-check --full "
                    "accepts. Commit, recovery, space, operations, transactions and the node cache match the model, "
                    "including the full-size forms the default run leaves out."),
            command="cargo test --include-ignored (tools/breenfs-host-tests)", unit="tests", planned=None,
            known_issues=KNOWN_ISSUES,
            list_argv=cargo_test(host, "--include-ignored", "--list"),
            run_argv=cargo_test(host, "--include-ignored", *report_time),
        ),
        LibtestStage(
            id="vfs", title="VFS model and conformance",
            checks=("The BreenFS writer behind the VFS matches the logical model on seeds 1-64, and the read and "
                    "write conformance, mount table and writable-owner checks pass, including the full-size forms "
                    "the default run leaves out."),
            command="cargo test --include-ignored --skip fill_and_delete (tools/vfs-host-tests)", unit="tests",
            planned=None,
            list_argv=cargo_test(vfs, "--include-ignored", *skip_fill, "--list"),
            run_argv=cargo_test(vfs, "--include-ignored", *skip_fill, *report_time),
        ),
        FillDeleteStage(
            id="fill-delete", title="Full-disk fill and delete",
            checks=("Sixteen seeded streams fill a BreenFS image until the reserve refuses writes with ENOSPC, "
                    "delete to free space and remount, three times over, matching the model exactly at every "
                    "refusal and remount."),
            command=f"cargo test --ignored --exact {FILL_DELETE_TEST} (tools/vfs-host-tests)", unit="seeds",
            planned=FILL_DELETE_SEEDS,
            list_argv=cargo_test(vfs, "--list"),
            run_argv=cargo_test(vfs, "--ignored", "--nocapture", *report_time, "--exact"),
        ),
        TortureStage(
            id="torture", title="Seeded crash torture",
            checks=("For each seed, every crash image of its seeded operation stream (subset, torn-old and "
                    "torn-garbage tails) recovers to an acknowledged commit that breenfs-check --full accepts, and "
                    "breenfs-check finds every single-bit flip of the populated image without recovery panicking."),
            command=f"breenfs-torture --seeds {torture_seeds} --checker breenfs-check (release)", unit="seeds",
            planned=torture_seeds, seeds=torture_seeds,
            builds=[(["cargo", "build", "--release", "--manifest-path", str(host), "--bin", "breenfs-torture"], REPO),
                    (["cargo", "build", "--release", "--manifest-path", str(core), "--bin", "breenfs-check"], REPO)],
            torture=lambda: cargo_target(host) / "release" / "breenfs-torture",
            checker=lambda: cargo_target(core) / "release" / "breenfs-check",
        ),
    ]


# --------------------------------------------------------------------------
# The run


@dataclass
class Captured:
    code: int
    output: str


class Battery:
    def __init__(self, root: Path, stages: list[Stage], profile: str, torture_seeds: int, repo: Path = REPO):
        self.root = root
        self.stages = stages
        self.profile = profile
        self.torture_seeds = torture_seeds
        self.repo = repo
        self.prefix = command_prefix()
        self.stopping = False
        self._stop_at = 0.0
        self._termed: set[int] = set()
        self._killed: set[int] = set()
        self._readers: dict[int, threading.Thread] = {}
        self._live: list[subprocess.Popen] = []
        self._started = False
        self._events_lock = threading.Lock()
        self._log_lock = threading.Lock()
        self._log = None
        self.run_dir = Path()
        self._events = None

    # -- stopping -------------------------------------------------------

    def request_stop(self, *_: object) -> None:
        if not self.stopping:
            self.stopping = True
            self._stop_at = time.monotonic()

    def supervise(self, procs: list[subprocess.Popen]) -> None:
        """Signal our own stage process groups once a stop is asked: SIGTERM, then SIGKILL after 30 s.

        Only the main thread reaps and signals, so a group is signalled only while
        its leader is unreaped and its process group id cannot have been reused.
        """
        if not self.stopping:
            return
        late = time.monotonic() - self._stop_at >= STOP_GRACE_SECONDS
        for proc in procs:
            if proc.poll() is not None:
                continue
            if proc.pid not in self._termed:
                self._termed.add(proc.pid)
                self._signal(proc, signal.SIGTERM)
            elif late and proc.pid not in self._killed:
                self._killed.add(proc.pid)
                self._signal(proc, signal.SIGKILL)

    @staticmethod
    def _signal(proc: subprocess.Popen, sig: int) -> None:
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            pass

    # -- records --------------------------------------------------------

    def event(self, event: str, **fields: object) -> None:
        line = {"at": now_iso(), "event": event, **fields}
        with self._events_lock:
            self._events.write(json.dumps(line) + "\n")
            self._events.flush()

    def note(self, text: str) -> None:
        self.event("note", text=text)

    def stage_start(self, stage: Stage, planned: Optional[int]) -> None:
        self._started = True
        self.event("stage-start", stage=stage.id, planned=planned)

    def case(self, stage: Stage, case: Case) -> None:
        fields: dict[str, object] = {"stage": stage.id, "name": case.name, "outcome": case.outcome}
        if case.seconds is not None:
            fields["seconds"] = case.seconds
        if case.outcome in ("fail", "known"):
            fields["message"] = case.message[:MESSAGE_LIMIT]
        self.event("case", **fields)

    def log_line(self, text: str) -> None:
        with self._log_lock:
            if self._log:
                self._log.write(text.encode("utf-8", "replace"))
                self._log.flush()

    # -- processes ------------------------------------------------------

    def spawn(self, stage: Stage, argv: list[str], cwd: Path, on_line: Callable[[str], None] = lambda _: None,
              prefix: str = "") -> subprocess.Popen:
        self.log_line(f"$ {' '.join(argv)}\n")
        proc = subprocess.Popen(
            self.prefix + argv, cwd=cwd, env=command_env(), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        )

        def read() -> None:
            assert proc.stdout is not None
            for raw in iter(proc.stdout.readline, b""):
                text = raw.decode("utf-8", "replace")
                self.log_line(prefix + text)
                try:
                    on_line(text.rstrip("\r\n"))
                except Exception as error:  # keep draining the pipe; record the parse fault
                    self.log_line(f"breenfs-battery: output handler: {type(error).__name__}: {error}\n")
            proc.stdout.close()

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        self._readers[proc.pid] = reader
        self._live.append(proc)
        return proc

    def join_output(self, proc: subprocess.Popen) -> None:
        reader = self._readers.pop(proc.pid, None)
        if reader:
            reader.join()

    def wait(self, proc: subprocess.Popen) -> int:
        while proc.poll() is None:
            self.supervise([proc])
            try:
                proc.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                pass
        self.join_output(proc)
        if self.stopping:
            raise Stopped()
        return proc.returncode

    def stream(self, stage: Stage, argv: list[str], cwd: Path, on_line: Callable[[str], None]) -> int:
        if self.stopping:
            raise Stopped()
        return self.wait(self.spawn(stage, argv, cwd, on_line))

    def capture(self, stage: Stage, argv: list[str], cwd: Path) -> Captured:
        lines: list[str] = []
        code = self.stream(stage, argv, cwd, lines.append)
        return Captured(code, "\n".join(lines))

    # -- the battery ----------------------------------------------------

    def git(self, *args: str) -> str:
        return subprocess.run(["git", "-C", str(self.repo), *args], capture_output=True, text=True,
                              check=True).stdout.strip()

    def describe_checkout(self) -> tuple[str, str, bool]:
        commit = self.git("rev-parse", "HEAD")
        branch = self.git("branch", "--show-current")
        if not branch:
            try:
                branch = "main" if self.git("rev-parse", "origin/main") == commit else "detached"
            except subprocess.CalledProcessError:
                branch = "detached"
        dirty = bool(self.git("status", "--porcelain"))
        return commit, branch, dirty

    def run(self) -> int:
        self.root.mkdir(parents=True, exist_ok=True)
        lock = open(self.root / ".lock", "a")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print(f"breenfs-battery: another battery is running (it holds {self.root / '.lock'})", file=sys.stderr)
            lock.close()
            return EXIT_LOCKED
        try:
            return self._run()
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)
            lock.close()

    def _run(self) -> int:
        commit, branch, dirty = self.describe_checkout()
        while True:
            started = _dt.datetime.now(_dt.timezone.utc)
            run_id = started.strftime("%Y%m%dT%H%M%SZ") + "-breenfs"
            self.run_dir = self.root / run_id
            try:
                self.run_dir.mkdir()
                break
            except FileExistsError:
                time.sleep(1)
        (self.run_dir / "stages").mkdir()
        write_json_atomic(self.run_dir / "run.json", {
            "schema": 1, "id": run_id, "battery": "breenfs", "commit": commit, "branch": branch,
            "dirty": dirty, "startedAt": started.strftime("%Y-%m-%dT%H:%M:%SZ"), "profile": self.profile,
            "tortureSeeds": self.torture_seeds, "pid": os.getpid(),
            "stages": [stage.describe() for stage in self.stages],
        })
        self._events = open(self.run_dir / "events.jsonl", "a", encoding="utf-8")
        print(f"breenfs-battery: {self.run_dir}", file=sys.stderr)
        results = []
        for stage in self.stages:
            if self.stopping:
                results.append({"id": stage.id, "outcome": "stopped", "seconds": 0.0, "done": 0, "failures": 0})
                continue
            results.append(self._stage(stage))
        outcomes = [r["outcome"] for r in results]
        if "fail" in outcomes:
            overall, code = "fail", 1
        elif self.stopping or "stopped" in outcomes:
            overall, code = "stopped", EXIT_STOPPED
        elif "known" in outcomes:
            overall, code = "known", 0
        else:
            overall, code = "pass", 0
        write_json_atomic(self.run_dir / "result.json", {
            "finishedAt": now_iso(), "outcome": overall, "exit": code, "stages": results,
        })
        self._events.close()
        print(f"breenfs-battery: {overall}", file=sys.stderr)
        return code

    def _end_children(self) -> None:
        """After a runner fault, end any stage process still running (our own groups only)."""
        alive = [p for p in self._live if p.poll() is None]
        if not alive:
            return
        deadline = time.monotonic() + STOP_GRACE_SECONDS
        for proc in alive:
            self._signal(proc, signal.SIGTERM)
        while any(p.poll() is None for p in alive) and time.monotonic() < deadline:
            time.sleep(0.2)
        for proc in alive:
            if proc.poll() is None:
                self._signal(proc, signal.SIGKILL)
                proc.wait()

    def _stage(self, stage: Stage) -> dict:
        clock = time.monotonic()
        self._started = False
        self._live = []
        self._log = open(self.run_dir / "stages" / f"{stage.id}.log", "ab")
        self.note(f"{stage.title}: building and starting ({stage.command})")
        try:
            finish = stage.run(self)
        except Stopped:
            finish = Finish("stopped", 0, 0, "stopped by the operator")
        except Exception as error:  # a runner fault fails this stage; later stages still run
            self._end_children()
            self.log_line(f"breenfs-battery: {type(error).__name__}: {error}\n")
            finish = Finish("fail", 0, 0, f"runner error: {type(error).__name__}: {error}"[:MESSAGE_LIMIT])
        finally:
            for proc in self._live:
                self.join_output(proc)
            self._log.close()
            self._log = None
        if not self._started:
            self.stage_start(stage, stage.planned)
        seconds = round(time.monotonic() - clock, 1)
        self.event("stage-finish", stage=stage.id, outcome=finish.outcome, seconds=seconds,
                   done=finish.done, failures=finish.failures, summary=finish.summary)
        return {"id": stage.id, "outcome": finish.outcome, "seconds": seconds, "done": finish.done,
                "failures": finish.failures}


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the opt-in breenfs validation battery.")
    parser.add_argument("--profile", choices=sorted(PROFILES), default="full")
    parser.add_argument("--torture-seeds", type=int, default=8, metavar="N")
    parser.add_argument("--stages", metavar="A,B", help=f"run these stages, in contract order: {','.join(STAGE_ORDER)}")
    parser.add_argument("--out", type=Path, default=DEFAULT_ROOT, help="batteries root (default: %(default)s)")
    args = parser.parse_args(argv)
    if not 1 <= args.torture_seeds <= 64:
        parser.error("--torture-seeds must be 1-64")
    if args.stages:
        asked = [s.strip() for s in args.stages.split(",") if s.strip()]
        unknown = [s for s in asked if s not in STAGE_ORDER]
        if unknown or not asked:
            parser.error(f"unknown stage {', '.join(unknown) or '(none)'}; stages are {', '.join(STAGE_ORDER)}")
        args.stage_ids = [s for s in STAGE_ORDER if s in asked]
    else:
        args.stage_ids = list(PROFILES[args.profile])
    return args


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    stages = [s for s in breenfs_stages(args.torture_seeds) if s.id in args.stage_ids]
    battery = Battery(args.out.expanduser(), stages, args.profile, args.torture_seeds)
    signal.signal(signal.SIGTERM, battery.request_stop)
    signal.signal(signal.SIGINT, battery.request_stop)
    return battery.run()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
