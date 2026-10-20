#!/usr/bin/env python3
"""Resolve and score boots that run several effort suites (docs/suites/README.md, "Several suites in one boot").

    scripts/suite-sequence.py resolve SPEC --target TARGET [--gpu-3d]
    scripts/suite-sequence.py score SUITES RECORDS_LOG [OTHER_LOG ...] [--disk IMAGE]
    scripts/suite-sequence.py run SUITES SERIAL LAUNCHER [ARG ...]

`resolve` prints the suite ids a boot runs, comma-separated, in order. SPEC is a suite id, ids joined by commas, or
`all`: every suite of docs/efforts/path.json the target runs (x86-metal runs every main-path effort's), in path order,
leaving out suites with no manifest or no cases yet and, on a target without a 3D GPU, suites that need one. TARGET is arm64-qemu,
parallels, vmware or x86-metal; ARM64 QEMU has a 3D GPU only with `--gpu-3d` (boot-interactive.sh --gpu-gl). In a list
of two or more, a manifest's `"sequence": "first"` suite must run first and its `"sequence": "last"` suite last.

`score` scores each suite of SUITES (comma-separated) from the serial as scripts/suite-verdict.py scores it alone,
and prints one line per suite: PASS, FAIL, STALLED (the runner's watchdog, or the boot, stopped it with the cases
shown), NOT-RUN (it never printed START) or NOT-ISOLATED (its RESET named state it could not restore; its own
verdict follows). Planned categories with no cases block PASS even when the populated categories pass.
The last suite's manifest diskChecks are read from --disk. Exits 0 only when every suite passes.
"""

import argparse
import json
import os
from pathlib import Path
import re
import runpy
import signal
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parent.parent
TARGETS = ("arm64-qemu", "parallels", "vmware", "x86-metal")
ID = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")


def read_repo(relative):
    path = REPO / relative
    return path.read_text() if path.is_file() else None


def gpu_3d(target, gpu_gl=False):
    """Whether the target has the 3D GPU a `needs3dGpu` suite renders on."""
    return target in ("parallels", "vmware") or (target == "arm64-qemu" and gpu_gl)


def resolve(spec, target, has_3d=False, read=read_repo):
    """The suite ids a boot runs, and the (id, reason) pairs `all` left out. Raises ValueError for a list it refuses."""
    if target not in TARGETS:
        raise ValueError(f"unknown target {target!r} (expected one of {', '.join(TARGETS)})")

    manifests = {}

    def manifest(suite):
        if suite not in manifests:
            text = read(f"docs/suites/{suite}.json")
            manifests[suite] = json.loads(text) if text else None
        return manifests[suite]

    def planned(suite):
        """Every category is planned (no cases yet), so the suite has no binary to boot."""
        return all(not category.get("cases") for category in manifest(suite).get("categories", []))

    skipped = []
    if spec == "all":
        ids = []
        for effort in json.loads(read("docs/efforts/path.json"))["efforts"]:
            suite = effort.get("suite")
            if not suite:
                continue
            if target != "x86-metal" and target not in effort.get("targets", []):
                skipped.append((suite, f"not an effort target of {target}"))
            elif manifest(suite) is None:
                skipped.append((suite, "no manifest"))
            elif planned(suite):
                skipped.append((suite, "no cases yet"))
            elif manifest(suite).get("needs3dGpu") and not has_3d:
                skipped.append((suite, "needs a 3D GPU"))
            elif suite not in ids:
                ids.append(suite)
        place = {suite: manifest(suite).get("sequence") for suite in ids}
        ids = ([s for s in ids if place[s] == "first"] + [s for s in ids if place[s] not in ("first", "last")]
               + [s for s in ids if place[s] == "last"])
    else:
        ids = spec.split(",")
        for suite in ids:
            if not ID.fullmatch(suite):
                raise ValueError(f"not a suite id (lowercase words joined by '-'): {suite!r}")
        if len(set(ids)) != len(ids):
            raise ValueError(f"a suite is listed twice: {spec}")
        for suite in ids:
            loaded = manifest(suite)
            if loaded is None:
                raise ValueError(f"no manifest docs/suites/{suite}.json")
            if planned(suite):
                raise ValueError(f"suite {suite} has no cases yet: every category in docs/suites/{suite}.json is planned")
            if loaded.get("needs3dGpu") and not has_3d:
                raise ValueError(f"suite {suite} needs a 3D GPU, which this {target} boot does not have")
    if not ids:
        raise ValueError(f"no suite to run on {target}")
    if len(ids) > 1:
        for index, suite in enumerate(ids):
            place = manifest(suite).get("sequence")
            # The kernel's console is not given back once a suite has taken the display, and a host disk check
            # reads the disk after the boot, so no suite may run after it.
            if place == "first" and index != 0:
                raise ValueError(f"suite {suite} needs the kernel's own console on screen, so it runs first "
                                 "(or in a boot of its own)")
            if place == "last" and index != len(ids) - 1:
                raise ValueError(f"suite {suite} checks the disk after the boot, so it runs last "
                                 "(or in a boot of its own)")
    return ids, skipped


def lines(text):
    """Complete lines only: a log write can stop anywhere in a record."""
    return [line.rstrip("\r") for line in text.split("\n")[:-1]]


def unreached(ids, text):
    """A failed DONE and a watchdog STALLED both consumed the suite; partial lines did not."""
    finished = set()
    for line in lines(text):
        match = re.fullmatch(r"SUITE ([a-z0-9-]+) DONE passed=\d+ failed=\d+ skipped=\d+ total=\d+", line)
        stalled = re.fullmatch(r"SUITE_SEQUENCE STALLED ([a-z0-9-]+) cases=\d+/\d+", line)
        if match or stalled:
            finished.add((match or stalled)[1])
    return [suite for suite in ids if suite not in finished]


def collect(ids, attempt):
    """Run boots serially, preserving failure status and retiring only suites with terminal evidence.

    attempt receives the ordered pending ids and attempt number; returns (serial text, exit status).
    """
    pending, status, number = list(ids), 0, 1
    while pending:
        text, code = attempt(pending, number)
        code = 128 - code if code < 0 else code
        status = status or code
        if 128 < code < 128 + signal.NSIG:
            print('suite-sequence: cancelled; no suites will resume', flush=True)
            return code
        remaining = unreached(pending, text)
        if not remaining:
            return status
        guest_death = any(re.search(r"KERNEL PANIC[:!]", line) or
                          (not line.startswith(("SUITE ", "SUITE_SEQUENCE ")) and
                           re.search(r"panicked at kernel/src/|!!! SOFT LOCKUP DETECTED !!!|"
                                     r"\[(?:DATA|INSTRUCTION)_ABORT\].*from_el0=0|"
                                     r"memory allocation of \d+ bytes failed|allocation error:", line))
                          for line in text.splitlines())
        if remaining == pending or not (code == 124 or guest_death):
            print(f"suite-sequence: stopped; could not get past {remaining[0]}; unreached: {','.join(remaining)}", flush=True)
            return status or 1
        print(f"suite-sequence: resuming unreached suites: {','.join(remaining)}", flush=True)
        pending, number = remaining, number + 1
    return status


def launch(suites, serial, command):
    """Supervise existing one-boot launchers; each attempt retains its serial, disk and Vigil filing."""
    serial = Path(serial).resolve()
    env = dict(os.environ, BREENIX_SUITE_ATTEMPT="1")

    def attempt(pending, number):
        log = serial if number == 1 else serial.parent / f"{serial.stem}-resume-{number}" / serial.name
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_bytes(b"")
        if number > 1:
            # Vigil owns the initial boot only; resumed boots need their own registrations.
            env.pop("VIGIL_BOOT_ID", None)
        args = command + ["--suite", ",".join(pending), "--serial-log", str(log)]
        if number > 1:
            args = [arg for arg in args if arg != "--clean"] + ["--no-build"]
        print(f"suite-sequence: boot {number}: {','.join(pending)}", flush=True)
        # subprocess.call kills its child on KeyboardInterrupt, before run.sh can
        # finish its EXIT trap. Forward cancellation and wait for synchronous cleanup.
        cancelled = 0
        child = None

        def cancel(signum, frame):
            nonlocal cancelled
            if cancelled:
                return
            cancelled = 128 + signum
            if child is not None and child.poll() is None:
                try:
                    os.killpg(child.pid, signum)
                except ProcessLookupError:
                    pass

        previous = {sig: signal.signal(sig, cancel)
                    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        try:
            child = subprocess.Popen(args, env=env, start_new_session=True)
            if cancelled:
                os.killpg(child.pid, cancelled - 128)
            code = child.wait()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)
        code = cancelled or code
        text = log.read_bytes().decode("utf-8", errors="replace") if log.exists() else ""
        return text, code

    return collect(suites.split(","), attempt)


def boundaries(text):
    """Count complete suite START and STALLED records."""
    return sum(bool(re.fullmatch(r"SUITE \S+ START cases=\d+|SUITE_SEQUENCE STALLED \S+ cases=\d+/\d+", line))
               for line in lines(text))


def windows(ids, records):
    """Each suite's part of the serial: from where it begins (the boot's start, for the first) to where the next
    suite begins (its RESET or START), so kernel output is held against the suite that was running."""
    starts = [0]
    for suite in ids[1:]:
        begins = (index for index, line in enumerate(records)
                  if line.startswith(f"SUITE_SEQUENCE RESET {suite} ") or re.fullmatch(rf"SUITE {suite} START cases=\d+", line))
        starts.append(next(begins, len(records)))
    # A suite that did not start ends where the one after it, or the serial, does.
    ends = [min(starts[i + 1:] + [len(records)]) for i in range(len(ids))]
    return [records[start:max(start, end)] for start, end in zip(starts, ends)]


def score(ids, records_log, logs, disk=None, repo=REPO):
    """(ok, rows): each row is (suite, status, detail)."""
    scorer = runpy.run_path(str(repo / "scripts/suite-verdict.py"))
    text = Path(records_log).read_bytes().decode("utf-8", errors="replace")
    rows = []
    with tempfile.TemporaryDirectory(prefix="suite-sequence-") as scratch:
        for index, (suite, records) in enumerate(zip(ids, windows(ids, lines(text)))):
            window = Path(scratch) / f"{suite}.txt"
            window.write_text("\n".join(records) + "\n")
            rows.append(score_one(scorer, repo, suite, suite == ids[-1], records, window,
                                  [window] + [Path(log) for log in logs[1:]], disk, index > 0))
    return all(status == "PASS" for _, status, _ in rows), rows


def score_one(scorer, repo, suite, last, records, window, logs, disk, needs_reset=False):
    manifest = json.loads((repo / "docs/suites" / f"{suite}.json").read_text())
    started = any(re.fullmatch(rf"SUITE {suite} START cases=\d+", line) for line in records)
    done = any(line.startswith(f"SUITE {suite} DONE ") for line in records)
    stalled = next((m for m in (re.fullmatch(rf"SUITE_SEQUENCE STALLED {suite} cases=(\d+)/(\d+)", line)
                                for line in records) if m), None)
    resets = [line[len("SUITE_SEQUENCE RESET "):] for line in records
              if line.startswith(f"SUITE_SEQUENCE RESET {suite} ")]
    # Missing, partial or failed reset evidence cannot establish isolation.
    unrestored = [reset for reset in resets if " FAIL " in reset
                  or not re.search(r" removed=\d+(?: |$)", reset)
                  or not re.search(r" changed=0(?: |$)", reset)
                  or not re.search(r" tcp=0(?: |$)", reset)
                  or not re.search(r" tcpwait=\d+(?: |$)", reset)]
    if needs_reset:
        before_start = records[:next((i for i, line in enumerate(records)
                                     if re.fullmatch(rf"SUITE {suite} START cases=\d+", line)), len(records))]
        if len(resets) != 1 or not any(line.startswith(f"SUITE_SEQUENCE RESET {suite} ") for line in before_start):
            unrestored.append("missing or ambiguous RESET before START")
    if not started:
        if resets and unrestored:
            return suite, "NOT-ISOLATED", f"RESET {'; '.join(unrestored)}; it never printed START"
        failed_exec = any(line == f"SUITE_SEQUENCE FAIL exec {suite}" for line in records)
        return suite, "NOT-RUN", "its binary could not be started" if failed_exec else "it never printed START"
    ok, detail = scorer["verdict"](manifest, window, logs)
    if stalled:
        status, detail = "STALLED", f"the runner's watchdog stopped it after {stalled[1]} of {stalled[2]} cases"
    elif not done and not detail.startswith("fatal kernel output"):
        reported = sum(bool(re.match(rf"SUITE {suite} CASE \S+ (?:PASS|FAIL|SKIP)", line)) for line in records)
        status, detail = "STALLED", f"the boot ended after {reported} of its cases: {detail}"
    else:
        if ok and last:
            ok, disk_detail = scorer["disk_verdict"](manifest.get("diskChecks", []), disk)
            if disk_detail:
                detail += f"; {disk_detail}"
        status = "PASS" if ok else "FAIL"
    if unrestored:
        status, detail = "NOT-ISOLATED", f"RESET {'; '.join(unrestored)}; its own result: {status}: {detail}"
    return suite, status, detail


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("resolve")
    p.add_argument("spec")
    p.add_argument("--target", required=True, choices=TARGETS)
    p.add_argument("--gpu-3d", action="store_true", help="ARM64 QEMU with the GL device (boot-interactive.sh --gpu-gl)")
    p = sub.add_parser("score")
    p.add_argument("suites")
    p.add_argument("logs", nargs="+")
    p.add_argument("--disk")
    p = sub.add_parser("run")
    p.add_argument("suites")
    p.add_argument("serial")
    p.add_argument("launcher", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command == "run":
        return launch(args.suites, args.serial, args.launcher)
    if args.command == "resolve":
        try:
            ids, skipped = resolve(args.spec, args.target, gpu_3d(args.target, args.gpu_3d))
        except ValueError as error:
            print(f"suite-sequence: {error}", file=sys.stderr)
            return 2
        for suite, why in skipped:
            print(f"suite-sequence: leaving out {suite}: {why}", file=sys.stderr)
        print(",".join(ids))
        return 0
    ids = args.suites.split(",")
    ok, rows = score(ids, args.logs[0], args.logs, args.disk)
    for suite, status, detail in rows:
        print(f"{status} {suite}: {detail}")
    passed = sum(status == "PASS" for _, status, _ in rows)
    print(f"{'PASS' if ok else 'FAIL'}: {passed} of {len(rows)} suites passed")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
