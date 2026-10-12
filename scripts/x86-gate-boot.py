#!/usr/bin/env python3
"""Run one full boot until completion, owning and reaping its process group."""
import os
import pathlib
import re
import signal
import subprocess
import sys
import time
import runpy

WallClockCheck = runpy.run_path(str(pathlib.Path(__file__).with_name("wall_clock.py")))["WallClockCheck"]

# The scoring deadline and the completion backstop serve different purposes.
# Full mode reports a missed scoring deadline after collecting the final report.
COMPLETE = re.compile(r'kernel::syscall::handlers: 🎯 USERSPACE TEST COMPLETE - All processes finished[^\r\n]*\r?$', re.M)
REPORT_DONE = re.compile(r'kernel::syscall::handlers: USERSPACE TEST REPORT DONE\r?$', re.M)
# The kernel's periodic census records, e.g. [TOMBSTONE_CENSUS:resident=0:removed=165],
# also reach COM1. They are not userspace progress.
CENSUS = re.compile(r'^\[[A-Z][A-Z0-9_]*(?::[a-z0-9_]+=[^:\]\r\n]*)+\]\r?$', re.M)


def userspace_output(text):
    """COM1's output without the kernel's census records."""
    return CENSUS.sub('', text)


def stop(process):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
            # The launcher can exit before its QEMU child; reap the whole group.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            return
        except subprocess.TimeoutExpired:
            continue
    process.wait()


def main():
    kernel_log = pathlib.Path(sys.argv[1])
    user_log = pathlib.Path(sys.argv[2])
    deadline = float(os.environ.get('BREENIX_GATE_TIMEOUT', '1800'))
    backstop = float(os.environ.get('BREENIX_FULL_BACKSTOP', str(max(1800, deadline))))
    silence = float(os.environ.get('BREENIX_USER_SILENCE', '120'))
    if deadline <= 0 or backstop <= 0 or silence <= 0:
        raise ValueError('gate deadline, full backstop and userspace silence limit must be positive')
    clock = WallClockCheck(user_log)
    process = subprocess.Popen(sys.argv[3:], start_new_session=True)
    started = time.monotonic()
    heard, last_heard = 0, started
    try:
        while True:
            text = kernel_log.read_text(errors='replace') if kernel_log.exists() else ''
            user = user_log.read_text(errors='replace') if user_log.exists() else ''
            clock.observe(user)
            if COMPLETE.search(text) and REPORT_DONE.search(text):
                elapsed = time.monotonic() - started
                print(f'[gate] full boot completed after {elapsed:.1f}s', flush=True)
                clock_ok, clock_reason = clock.verdict()
                print(clock_reason, flush=True)
                if elapsed > deadline:
                    print(f'GATE: FAIL (full boot completed beyond {deadline:g}s scoring deadline; #1068)', flush=True)
                    return 2
                return 0 if clock_ok else 3
            if process.poll() is not None:
                print(f'GATE: FAIL (full boot exited before USERSPACE TEST COMPLETE; status={process.returncode})', flush=True)
                return 1
            now = time.monotonic()
            output = len(userspace_output(user))
            if output != heard:
                heard, last_heard = output, now
            if now - started >= backstop or now - last_heard >= silence:
                if now - started >= backstop:
                    print(f'GATE: FAIL (full boot reached {backstop:g}s hang backstop; completion report still unfinished)', flush=True)
                else:
                    print(f'GATE: FAIL (userspace printed nothing but census records for {silence:g}s, '
                          f'{now - started:.0f}s into the boot; completion report still unfinished)', flush=True)
                # Existing strand records name the threads still saved in waits.
                subprocess.run([str(pathlib.Path(__file__).with_name('x86-strand-census.sh')),
                                str(user_log), str(kernel_log)], check=False)
                print('[gate] last kernel progress:', flush=True)
                print('\n'.join(text.splitlines()[-20:]), flush=True)
                print('[gate] last userspace progress (program output, not loader announcements):', flush=True)
                if user_log.exists():
                    user_text = user_log.read_text(errors='replace')
                    # These are contracts with a userspace START but no terminal
                    # marker, not an assertion that the loader ever ran them.
                    pending = set()
                    for name, state in re.findall(r'\b([A-Z][A-Z0-9_]*_TEST)_(START|PASSED|FAILED)\b', user_text):
                        if state == 'START':
                            pending.add(name)
                        else:
                            pending.discard(name)
                    print('[gate] unfinished started program contracts: ' +
                          (', '.join(sorted(pending)) or '<none with a START/terminal marker pair>'), flush=True)
                    print('\n'.join(user_text.splitlines()[-20:]), flush=True)
                return 1
            time.sleep(0.25)
    finally:
        stop(process)


if __name__ == '__main__':
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, lambda number, frame: sys.exit(128 + number))
    sys.exit(main())
