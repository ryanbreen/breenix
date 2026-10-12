"""Exercise run.sh's EXIT and signal cleanup with an actual log watcher."""

import os
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "run.sh"
MONITOR = ROOT / "scripts" / "vm-log-monitor.py"


class RunnerProcessCleanupTest(unittest.TestCase):
    def test_parallels_sweep_finishes_before_runner_exits(self):
        source = RUNNER.read_text()
        sweep = source.split('    PARALLELS_VM="breenix-$(date +%s)"', 1)[1]
        sweep = sweep.split('    echo "Creating fresh VM', 1)[0]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child_pid = root / "delete.pid"
            script = f"""
set -e
PARALLELS_VM=breenix-new
bounded_vm_command() {{ "$@"; }}
prlctl() {{
    if [ "$1" = list ]; then
        printf 'stopped breenix-123\\n'
    elif [ "$1" = delete ]; then
        sleep 1 &
        echo "$!" > {child_pid}
        wait "$!"
    fi
}}
{sweep}
"""
            with (root / "runner.out").open("w") as output:
                process = subprocess.Popen(["/bin/bash", "-c", script], stdout=output, stderr=output)
                self.assertEqual(process.wait(timeout=5), 0)
            pid = int(child_pid.read_text())
            self.assertFalse(self.is_alive(pid), f"Parallels sweep left PID {pid} alive")

    def test_exit_and_interruption_reap_log_monitor(self):
        source = RUNNER.read_text()
        finish = source.split("finish_vm_boot() {\n", 1)[1].split("\n}\n", 1)[0]
        install = source.split("install_vm_boot_traps() {\n", 1)[1].split("\n}\n", 1)[0]
        cases = (
            ("vmware_suite", "vmware", 0),
            ("success", "parallels", 0),
            ("failure", "parallels", 7),
            ("timeout", "parallels", 124),
            ("term", "parallels", 143),
            ("int", "parallels", 130),
            ("term", "vmware", 143),
            ("int", "vmware", 130),
        )
        for name, platform, exit_code in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "scripts").mkdir()
                (root / "scripts" / "vigil-record.sh").write_text("#!/bin/sh\nexit 0\n")
                (root / "scripts" / "vigil-record.sh").chmod(0o755)
                log = root / "vmware.log"
                log.write_text("started\n")
                pid_file = root / "monitor.pid"
                script = f"""
finish_vm_boot() {{
{finish}
}}
install_vm_boot_traps() {{
{install}
}}
prlctl() {{ :; }}
host_slot_header() {{ :; }}
vmware_check_network_log() {{ :; }}
vmware_network_summary() {{ :; }}
sweep_vmware_bundles() {{ :; }}
PARALLELS={'true' if platform == 'parallels' else 'false'}
PARALLELS_VM=test-vm
VMRUN=/usr/bin/true
VMX_FILE={root / 'test.vmx'}
VM_BUNDLE={root}
BREENIX_ROOT={root}
SERIAL_LOG={root / 'serial.log'}
python3 -u {MONITOR} {platform} {log} "$$" &
LOGMON_PID=$!
echo "$LOGMON_PID" > {pid_file}
install_vm_boot_traps
{f'wait "$LOGMON_PID"' if name in {'int', 'term'} else f'exit {exit_code}'}
"""
                process = subprocess.Popen(["/bin/bash", "-c", script])
                pid = self.read_pid(pid_file)
                if name in {"int", "term"}:
                    process.send_signal(signal.SIGINT if name == "int" else signal.SIGTERM)
                self.assertEqual(process.wait(timeout=5), exit_code)
                deadline = time.monotonic() + 3
                while self.is_alive(pid) and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertFalse(self.is_alive(pid), f"{name} left monitor PID {pid} alive")

    def test_monitor_exits_if_parent_is_killed(self):
        for platform in ("parallels", "vmware"):
            for create_log in (False, True):
                with self.subTest(platform=platform, log=create_log), tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    log, pid_file = root / "vm.log", root / "pid"
                    if create_log:
                        log.write_text("old fault\n")
                    process = subprocess.Popen(["/bin/bash", "-c", f'python3 -u {MONITOR} {platform} {log} "$$" & echo $! > {pid_file}; wait'])
                    pid = self.read_pid(pid_file)
                    time.sleep(0.2)
                    process.kill()
                    process.wait(timeout=3)
                    deadline = time.monotonic() + 3
                    while self.is_alive(pid) and time.monotonic() < deadline:
                        time.sleep(0.02)
                    self.assertFalse(self.is_alive(pid), f"parent SIGKILL left monitor {pid}")

    def test_monitor_initial_window_and_match_order(self):
        for platform in ("parallels", "vmware"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temporary:
                log = Path(temporary) / "vm.log"
                log.write_text("old fault\n" + "ordinary\n" * 10 + "new fault\nexception before vcpu\nvcpu then exception\n")
                process = subprocess.Popen(["python3", "-u", str(MONITOR), platform, str(log), str(os.getpid())], stdout=subprocess.PIPE, text=True)
                time.sleep(0.2)
                process.terminate()
                output, _ = process.communicate(timeout=3)
                self.assertNotIn("old fault", output)
                self.assertIn("new fault", output)
                if platform == "vmware":
                    self.assertNotIn("exception before vcpu", output)
                    self.assertIn("vcpu then exception", output)

    def test_maintenance_timeout_reaps_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            pid_file = Path(temporary) / "pid"
            process = subprocess.Popen(["python3", str(ROOT / "scripts/bounded-vm-command.py"), "0.3", "/bin/sh", "-c", f"sleep 30 & echo $! > {pid_file}; wait"])
            pid = self.read_pid(pid_file)
            self.assertEqual(process.wait(timeout=3), 124)
            deadline = time.monotonic() + 3
            while self.is_alive(pid) and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertFalse(self.is_alive(pid))

    @staticmethod
    def read_pid(path):
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                contents = path.read_text().strip()
                if contents:
                    return int(contents)
            except FileNotFoundError:
                pass
            time.sleep(0.02)
        raise AssertionError(f"no complete PID in {path}")

    @staticmethod
    def is_alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True


if __name__ == "__main__":
    unittest.main()
