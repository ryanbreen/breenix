#!/usr/bin/env python3
"""Boot ARM64 shell mode and check serial chvt switching with screen read-back.

Build first, then run this script with --no-build. Without that option the
launcher builds with background priority and four Cargo jobs. The launcher owns
its mac-boot lease, window, disposable disk, Vigil registration and VM cleanup.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def wait_for(serial, process, predicate, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        text = serial.read_text(errors="replace") if serial.exists() else ""
        # Only terminal colour/cursor controls are removed; line boundaries stay.
        text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", text).replace("\r", "")
        result = predicate(text)
        if result is not None:
            return result
        if process.poll() is not None:
            raise RuntimeError(f"shell boot exited {process.returncode} before read-back")
        time.sleep(0.1)
    raise RuntimeError("shell boot did not produce the expected read-back before the deadline")


def check(args):
    directory = Path(args.output).resolve() if args.output else Path(tempfile.mkdtemp(prefix="console-vt-shell-"))
    directory.mkdir(parents=True, exist_ok=True)
    serial = directory / "serial.txt"
    serial.unlink(missing_ok=True)
    command = [str(ROOT / "scripts/boot-interactive.sh"), "--mode", "shell", "--display",
               "--serial-log", str(serial), "--idle-exit", "90"]
    if args.no_build:
        command.append("--no-build")
    environment = dict(os.environ, CARGO_BUILD_JOBS="4", BREENIX_BUILD_BACKGROUND="1")
    with (directory / "launcher.txt").open("wb") as output:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdin=subprocess.PIPE,
                                   stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            wait_for(serial, process, lambda text: True if re.search(r"bsh[^\n]*> ", text) else None, args.timeout)
            for number in (3, 1):
                begin, end = f"VT-SERIAL-BEGIN-{number}", f"VT-SERIAL-END-{number}"
                # bsh is ECMAScript. Its exec return status is checked before the
                # read-back; printing readFile's result retains producer lines.
                statements = []
                if number == 3:
                    statements.append('writeFile("/dev/tty3", "\\u001bcserial-vt3\\r\\n");')
                statements.append(f'if (exec("/bin/chvt", "{number}").exitCode === 0) {{ '
                                  f'print("\\n{begin}\\n"); print(readFile("/proc/console/check")); '
                                  f'print("\\n{end}\\n"); }}')
                process.stdin.write((" ".join(statements) + "\n").encode())
                process.stdin.flush()

                def frame(text):
                    match = re.search(rf"(?m)^{begin}\n(.*?)^{end}$", text, re.S)
                    return match.group(1).splitlines() if match else None

                lines = wait_for(serial, process, frame, 30)
                required = [f"console foreground: tty{number}", "console check: shadow and screen identical"]
                if number == 3:
                    required.append("console row: serial-vt3")
                for line in required:
                    if line not in lines:
                        raise RuntimeError(f"missing producer line: {line}")
                if not any(line.startswith("console frame: ") and line.endswith(" complete") for line in lines):
                    raise RuntimeError("read-back did not name a completed frame")
                print(f"serial chvt: tty{number} read back identical", flush=True)
            print("SERIAL CHVT DONE passed=2 failed=0", flush=True)
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=120)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    print(json.dumps({"serial": str(serial)}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--output", help="private run directory")
    parser.add_argument("--timeout", type=int, default=1800, help="build and shell startup deadline in seconds")
    try:
        check(parser.parse_args())
    except (OSError, RuntimeError) as error:
        print(f"serial chvt: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
