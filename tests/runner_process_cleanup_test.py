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
prlctl() {{
    if [ "$1" = list ]; then
        printf 'stopped breenix-123\\n'
    elif [ "$1" = delete ]; then
        sleep 0.25 &
        echo "$!" > {child_pid}
        wait "$!"
    fi
}}
{sweep}
"""
            process = subprocess.run(["/bin/bash", "-c", script], capture_output=True, timeout=5)
            self.assertEqual(process.returncode, 0, process.stderr.decode())
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
            ("interruption", "parallels", 143),
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
python3 -u {MONITOR} vmware {log} &
LOGMON_PID=$!
echo "$LOGMON_PID" > {pid_file}
install_vm_boot_traps
{f'wait "$LOGMON_PID"' if name == 'interruption' else f'exit {exit_code}'}
"""
                process = subprocess.Popen(["/bin/bash", "-c", script])
                deadline = time.monotonic() + 3
                while not pid_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                pid = int(pid_file.read_text())
                if name == "interruption":
                    process.send_signal(signal.SIGTERM)
                self.assertEqual(process.wait(timeout=5), exit_code)
                deadline = time.monotonic() + 3
                while self.is_alive(pid) and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertFalse(self.is_alive(pid), f"{name} left monitor PID {pid} alive")

    @staticmethod
    def is_alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True


if __name__ == "__main__":
    unittest.main()
