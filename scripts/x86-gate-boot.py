#!/usr/bin/env python3
"""Run one full boot until completion, owning and reaping its process group."""
import os
import pathlib
import re
import signal
import subprocess
import sys
import time

# bf95eeb6 needs about 340 seconds with real BusyBox execution (#1068).
# 1800 seconds is over five times that measurement, solely a hang backstop.
# The older caller-supplied 300-second gate timeout must not cut off full mode.
BACKSTOP_SECONDS = 1800
COMPLETE = re.compile(r'^\[ INFO\] kernel::syscall::handlers: 🎯 USERSPACE TEST COMPLETE - All processes finished[^\r\n]*\r?$', re.M)


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
    process = subprocess.Popen(sys.argv[3:], start_new_session=True)
    started = time.monotonic()
    try:
        while True:
            text = kernel_log.read_text(errors='replace') if kernel_log.exists() else ''
            if COMPLETE.search(text):
                # Completion precedes the tally and final existing diagnostics.
                # Drain that trailing output before stopping the idle kernel.
                time.sleep(2)
                print(f'[gate] full boot completed after {time.monotonic()-started:.1f}s', flush=True)
                return 0
            if process.poll() is not None:
                print(f'GATE: FAIL (full boot exited before USERSPACE TEST COMPLETE; status={process.returncode})', flush=True)
                return 1
            if time.monotonic() - started >= BACKSTOP_SECONDS:
                print(f'GATE: FAIL (full boot reached {BACKSTOP_SECONDS}s hang backstop; userspace testing still unfinished)', flush=True)
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
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, lambda number, frame: sys.exit(128 + number))
    sys.exit(main())
