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
BootProgress = runpy.run_path(str(pathlib.Path(__file__).with_name("boot_progress.py")))["BootProgress"]
relay_api = runpy.run_path(str(pathlib.Path(__file__).with_name("x86_serial_relay.py")))

# The scoring deadline and the completion backstop serve different purposes.
# Full mode reports a missed scoring deadline after collecting the final report.
COMPLETE = re.compile(r'kernel::syscall::handlers: 🎯 USERSPACE TEST COMPLETE - All processes finished[^\r\n]*\r?$', re.M)
REPORT_DONE = re.compile(r'kernel::syscall::handlers: USERSPACE TEST REPORT DONE\r?$', re.M)


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


def diagnose(user_log, kernel_log, text):
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

def main():
    kernel_log = pathlib.Path(sys.argv[1])
    user_log = pathlib.Path(sys.argv[2])
    deadline = float(os.environ.get('BREENIX_GATE_TIMEOUT', '1800'))
    backstop = float(os.environ.get('BREENIX_FULL_BACKSTOP', str(max(1800, deadline))))
    if deadline <= 0 or backstop <= 0:
        raise ValueError('gate deadline and full backstop must be positive')
    started = time.monotonic()
    progress = BootProgress(user_log, now=started)
    clock = WallClockCheck(user_log)
    command, relays = relay_api['live_serial_command'](sys.argv[3:], (user_log, kernel_log))
    # On macOS this launcher also serves windowed lane diagnostics outside breenix-runs. Register before launch so
    # architecture and the tracked-change snapshot describe what actually boots, not the checkout after it finishes.
    recorder = pathlib.Path(__file__).with_name('vigil-record.sh')
    record_env = dict(os.environ, BREENIX_VIGIL_ARCH='x86_64', BREENIX_LAUNCHER_PID=str(os.getpid()))
    if '-smp' in command:
        cpus = command[command.index('-smp') + 1]
        if cpus != '1':
            record_env['BREENIX_VIGIL_CPUS'] = cpus
    boot_id = ''
    if recorder.is_file():
        registered = subprocess.run([str(recorder), 'start', 'qemu', 'tests', os.environ.get('BREENIX_BOOT_SUITE', ''),
                                     str(kernel_log), str(user_log), os.environ.get('BREENIX_QEMU_PROFILE', '')],
                                    env=record_env, capture_output=True, text=True, check=False)
        if registered.returncode == 0:
            boot_id = registered.stdout.strip()
    result = 1
    try:
        process = subprocess.Popen(command, start_new_session=True)
    except BaseException:
        relay_api['close_relays'](relays)
        if boot_id:
            subprocess.run([str(recorder), 'finish', boot_id, '1'], env=record_env, check=False)
        raise
    try:
        while True:
            text = kernel_log.read_text(errors='replace') if kernel_log.exists() else ''
            progress.poll([(kernel_log, False), (user_log, True)])
            clock.observe(user_log.read_text(errors='replace') if user_log.exists() else '')
            if COMPLETE.search(text) and REPORT_DONE.search(text):
                elapsed = time.monotonic() - started
                print(f'[gate] full boot completed after {elapsed:.1f}s', flush=True)
                clock_ok, clock_reason = clock.verdict()
                print(clock_reason, flush=True)
                if elapsed > deadline:
                    print(f'GATE: FAIL (full boot completed beyond {deadline:g}s scoring deadline; #1068)', flush=True)
                    result = 2
                    return result
                result = 0 if clock_ok else 3
                return result
            if progress.stalled():
                progress.record()
                diagnose(user_log, kernel_log, text)
                result = 124
                return result
            if process.poll() is not None:
                print(f'GATE: FAIL (full boot exited before USERSPACE TEST COMPLETE; status={process.returncode})', flush=True)
                return 1
            if time.monotonic() - started >= backstop:
                print(f'GATE: FAIL (full boot reached {backstop:g}s hang backstop; completion report still unfinished)', flush=True)
                diagnose(user_log, kernel_log, text)
                return 1
            time.sleep(0.25)
    finally:
        try:
            stop(process)
        finally:
            relay_api['close_relays'](relays)
            if boot_id:
                error = sys.exc_info()[1]
                status = error.code if isinstance(error, SystemExit) else result
                subprocess.run([str(recorder), 'finish', boot_id, str(status)], env=record_env, check=False)


if __name__ == '__main__':
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, lambda number, frame: sys.exit(128 + number))
    sys.exit(main())
