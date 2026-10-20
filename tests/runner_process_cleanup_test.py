"""Exercise run.sh's EXIT and signal cleanup with an actual log watcher."""

import json
import os
import re
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
    def test_owned_parallels_deletion_finishes_before_runner_exits(self):
        source = RUNNER.read_text()
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', source))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            child_pid = root / "delete.pid"
            script = "\n".join(f"{name}() {{\n{functions[name]}}}" for name in
                               ("finish_vm_boot", "delete_parallels_vm", "install_vm_boot_traps"))
            script += f"""
bounded_vm_command() {{ "$@"; }}
prlctl() {{
    if [ "$1" = list ]; then
        printf '  hdd0 (+) image\\n  hdd1 (+) image\\n'
    elif [ "$1" = delete ]; then
        sleep 1 &
        echo "$!" > {child_pid}
        wait "$!"
    fi
}}
host_slot_header() {{ :; }}
host_slot_release() {{ :; }}
BREENIX_ROOT={ROOT}
PARALLELS=true
PARALLELS_VM=breenix-owned
SERIAL_LOG={root / 'serial'}
install_vm_boot_traps
exit 0
"""
            process = subprocess.Popen(["/bin/bash", "-c", script])
            self.assertEqual(process.wait(timeout=5), 0)
            pid = int(child_pid.read_text())
            self.assertFalse(self.is_alive(pid), f"Owned Parallels deletion left PID {pid} alive")

    def test_dead_owner_sweeps_preserve_live_owners_and_reap_deletion(self):
        source = RUNNER.read_text()
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', source))
        names = ("vm_run_owner", "delete_parallels_vm", "sweep_parallels_vms",
                 "vmware_bundle_stopped", "delete_vmware_bundle", "sweep_vmware_bundles")
        definitions = "\n".join(f"{name}() {{\n{functions[name]}}}" for name in names)
        for platform in ("parallels", "vmware"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                markers = root / "owners"
                markers.mkdir()
                vmrun = root / "vmrun"
                vmrun.write_text('#!/bin/bash\n' + f"""
if [ "$1" = list ]; then exit 0; fi
[ "$1" = deleteVM ] || exit 1
sleep 1 &
echo "$!" > {root / 'delete.pid'}
wait "$!"
echo "$2" >> {root / 'deleted'}
rm -f "$2"
""")
                vmrun.chmod(0o755)
                dead = subprocess.Popen(["sleep", "30"])
                try:
                    for name, pid in (("live", os.getpid()), ("dead", dead.pid),
                                      ("running", dead.pid), ("reused", os.getpid())):
                        vm = "breenix-" + name
                        if platform == "parallels":
                            marker = markers / (vm + ".json")
                        else:
                            bundle = root / (vm + ".vmwarevm")
                            bundle.mkdir()
                            (bundle / ".breenix-run-owner").write_text(vm + "\n")
                            (bundle / (vm + ".vmx")).write_text("guest")
                            marker = bundle / ".breenix-run-process"
                        subprocess.run(["python3", str(ROOT / "scripts/vm-run-owner.py"),
                                        "record", str(marker), str(pid)], check=True)
                        if name == "reused":
                            owner = json.loads(marker.read_text())
                            owner["started"] = "previous process"
                            marker.write_text(json.dumps(owner))
                    dead.terminate()
                    dead.wait(timeout=3)
                    script = definitions + f"""
set -e
BREENIX_ROOT={ROOT}
PARALLELS_OWNER_DIR={markers}
VM_MACHINES={root}
VMRUN={vmrun}
bounded_vm_command() {{ "$@"; }}
prlctl() {{
    if [ "$1" = status ]; then
        if [ "$2" = breenix-running ]; then echo "$2 running"; else echo "$2 stopped"; fi
    elif [ "$1" = list ]; then
        printf '  hdd0 (+) image\\n  hdd1 (+) image\\n'
    elif [ "$1" = delete ]; then
        sleep 1 &
        echo "$!" > {root / 'delete.pid'}
        wait "$!"
        echo "$2" >> {root / 'deleted'}
    fi
}}
"""
                    # Report the dead owner's running VM as still running on Fusion.
                    if platform == "vmware":
                        text = vmrun.read_text().replace('then exit 0; fi',
                            f'then echo "{root}/breenix-running.vmwarevm/breenix-running.vmx"; exit 0; fi', 1)
                        vmrun.write_text(text)
                    result = subprocess.run(["/bin/bash", "-c", script + f"sweep_{platform}_" +
                                             ("vms" if platform == "parallels" else "bundles")],
                                            capture_output=True, text=True, timeout=8)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    deleted = (root / "deleted").read_text().splitlines()
                    self.assertEqual(len(deleted), 2, deleted)
                    self.assertTrue(any("breenix-dead" in line for line in deleted), deleted)
                    self.assertTrue(any("breenix-reused" in line for line in deleted), deleted)
                    self.assertFalse(any("breenix-live" in line or "breenix-running" in line for line in deleted))
                    self.assertFalse(self.is_alive(int((root / "delete.pid").read_text())))
                    if platform == "parallels":
                        self.assertTrue((markers / "breenix-live.json").exists())
                        self.assertFalse((markers / "breenix-dead.json").exists())
                    else:
                        self.assertTrue((root / "breenix-live.vmwarevm/breenix-live.vmx").exists())
                        self.assertFalse((root / "breenix-dead.vmwarevm").exists())
                finally:
                    if dead.poll() is None:
                        dead.terminate()
                        dead.wait(timeout=3)

    def test_exit_and_interruption_reap_log_monitor(self):
        source = RUNNER.read_text()
        finish = source.split("finish_vm_boot() {\n", 1)[1].split("\n}\n", 1)[0]
        stopped = source.split("vmware_bundle_stopped() {\n", 1)[1].split("\n}\n", 1)[0]
        install = source.split("install_vm_boot_traps() {\n", 1)[1].split("\n}\n", 1)[0]
        cases = (
            ("vmware_suite", "vmware", 0),
            ("already_off", "vmware", 0),
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
                (root / "vmrun").write_text("#!/bin/sh\n" + (
                    "[ \"$1\" != stop ]\n" if name == "already_off" else "exit 0\n"))
                (root / "vmrun").chmod(0o755)
                log = root / "vmware.log"
                log.write_text("started\n")
                pid_file = root / "monitor.pid"
                keys_pid_file = root / "hostkeys.pid"
                deleted = root / "deleted"
                script = f"""
finish_vm_boot() {{
{finish}
}}
install_vm_boot_traps() {{
{install}
}}
bounded_vm_command() {{ "$@"; }}
vmware_bundle_stopped() {{
{stopped}
}}
prlctl() {{ :; }}
host_slot_header() {{ :; }}
vmware_check_network_log() {{ :; }}
vmware_network_summary() {{ :; }}
delete_parallels_vm() {{ :; }}
prune_vmware_inventory() {{ :; }}
host_slot_release() {{ :; }}
delete_vmware_bundle() {{ echo "$1" > {deleted}; }}
PARALLELS={'true' if platform == 'parallels' else 'false'}
PARALLELS_VM=test-vm
VMRUN={root / 'vmrun'}
VMX_FILE={root / 'test.vmx'}
VM_BUNDLE={root}
BREENIX_ROOT={root}
SERIAL_LOG={root / 'serial.log'}
python3 -u {MONITOR} {platform} {log} "$$" &
LOGMON_PID=$!
echo "$LOGMON_PID" > {pid_file}
python3 {ROOT / "scripts/hostkeys.py"} watch --serial "$SERIAL_LOG" --backend none --parent "$$" &
HOSTKEYS_PID=$!
echo "$HOSTKEYS_PID" > {keys_pid_file}
install_vm_boot_traps
{f'wait "$LOGMON_PID"' if name in {'int', 'term'} else f'exit {exit_code}'}
"""
                process = subprocess.Popen(["/bin/bash", "-c", script])
                pid = self.read_pid(pid_file)
                keys_pid = self.read_pid(keys_pid_file)
                if name in {"int", "term"}:
                    process.send_signal(signal.SIGINT if name == "int" else signal.SIGTERM)
                self.assertEqual(process.wait(timeout=5), exit_code)
                deadline = time.monotonic() + 3
                while self.is_alive(pid) and time.monotonic() < deadline:
                    time.sleep(0.02)
                self.assertFalse(self.is_alive(pid), f"{name} left monitor PID {pid} alive")
                self.assertFalse(self.is_alive(keys_pid), f"{name} left hostkeys PID {keys_pid} alive")
                if platform == "vmware":
                    self.assertEqual(deleted.read_text().strip(), str(root), f"{name} kept its VM")
                else:
                    self.assertFalse(deleted.exists(), f"{name} deleted a VMware bundle")

    def test_stopped_suite_exits_before_log_watching(self):
        source = RUNNER.read_text()
        blocks = re.findall(r'    if \[ "\$BOOT_MODE" = probe \] \|\| \[ "\$BOOT_MODE" = suite \] \|\| \[ "\$BOOT_MODE" = tests \](?: \|\| \[ "\$BOOT_MODE" = program \])?; then\n.*?\n    fi', source, re.S)
        self.assertEqual(len(blocks), 2)
        for block in blocks:
            script = "BOOT_MODE=suite; PARALLELS_TEST=false; wait_boot_done() { :; }; hold_suite_panel() { :; }; vmware_check_network_log() { :; }; vmware_network_summary() { :; }; " + block + "\necho unexpected-watcher"
            result = subprocess.run(["/bin/bash", "-c", script], capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertNotIn(b"unexpected-watcher", result.stdout)

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
