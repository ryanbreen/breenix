"""Exercise production monitor exits and cleanup with synthetic serials."""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
INJECT = '''import runpy,sys
script=sys.argv[1]; argv=sys.argv[2:]
m=runpy.run_path(script); g=m['main'].__globals__
Base=g['BootProgress']
class FastProgress(Base):
    def __init__(self,*a,**kw):
        kw['timeout']=0.5
        super().__init__(*a,**kw)
g['BootProgress']=FastProgress
sys.argv=[script]+argv
raise SystemExit(m['main']())
'''
REPLAY = '''from pathlib import Path
import signal,sys,time
user,kernel,stopped=map(Path,sys.argv[1:])
def stop(*args):
    stopped.write_text('reaped'); sys.exit(0)
signal.signal(signal.SIGTERM,stop)
user.write_text('SIGKILL_TEARDOWN_TEST: FAIL\\n')
for i in range(200):
    with kernel.open('a') as f: f.write('[DISPATCH_STRAND_CENSUS:seq=%d]\\n'%i)
    time.sleep(0.05)
'''


class StallLaunchers(unittest.TestCase):
    def test_native_done_stops_guest_and_records_success(self):
        source = (ROOT / 'run.sh').read_text()
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', source))
        definitions = '\n'.join(f'{name}() {{\n{functions[name]}}}' for name in
                                ('wait_boot_done', 'bounded_vm_command', 'finish_vm_boot',
                                 'install_vm_boot_traps', 'hold_suite_panel', 'vm_is_running')
                                if name in functions)
        blocks = re.findall(r'(?ms)^    if \[ "\$BOOT_MODE" = probe \].*?^    fi$', source)
        self.assertEqual(len(blocks), 2)
        manifest = json.loads((ROOT / 'docs/suites/smoke.json').read_text())
        cases = [f"{category['id']}/{case['id']}" for category in manifest['categories']
                 for case in category['cases']]
        for index, platform in enumerate(('parallels', 'vmware')):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp); scripts = out / 'scripts'; scripts.mkdir()
                for name in ('wait-boot-done.py', 'bounded-vm-command.py', 'wall_clock.py', 'boot_progress.py'):
                    (scripts / name).symlink_to(ROOT / 'scripts' / name)
                recorder = scripts / 'vigil-record.sh'
                recorder.write_text('#!/bin/sh\nprintf "%s" "$3" > "$RECORD"\n')
                recorder.chmod(0o755)
                serial, stopped, record = (out / name for name in ('serial', 'stopped', 'record'))
                guest_code = '''import pathlib,signal,sys,time
serial,stopped=map(pathlib.Path,sys.argv[1:3])
def stop(*args):
 stopped.write_text('stopped');sys.exit(0)
signal.signal(signal.SIGTERM,stop)
serial.write_text('[clock] UTC epoch=%d source=UEFI\\n'%time.time()+sys.argv[3])
while True:time.sleep(.1)
'''
                text = 'SUITE smoke START cases=5\n' + ''.join(
                    f'SUITE smoke CASE {case} PASS ms=1\n' for case in cases)
                text += 'SUITE smoke DONE passed=5 failed=0 skipped=0 total=5\n'
                started = time.time()
                guest = subprocess.Popen([sys.executable, '-c', guest_code, str(serial), str(stopped), text])
                controller = out / 'vmrun'
                controller.write_text('#!/bin/sh\nif [ "$1" = stop ]; then kill -TERM "$GUEST_PID"; else echo "$VMX_FILE"; fi\n')
                controller.chmod(0o755)
                env = dict(os.environ, BREENIX_ROOT=str(out), SERIAL_LOG=str(serial),
                           BOOT_MODE='suite', SUITE='smoke', GATE_TIMEOUT='10',
                           CLOCK_STARTED=str(started), PARALLELS=str(platform == 'parallels').lower(),
                           PARALLELS_TEST='false', PARALLELS_VM='fake', VMX_FILE='fake.vmx',
                           VMRUN=str(controller), VM_BUNDLE=str(out), GUEST_PID=str(guest.pid),
                           VIGIL_ID='fake', RECORD=str(record))
                stubs = '''prlctl() { if [ "$1" = stop ]; then kill -TERM "$GUEST_PID"; else echo running; fi; }
host_slot_header() { :; }
vmware_check_network_log() { :; }
vmware_network_summary() { :; }
sweep_vmware_bundles() { sleep 10; }
'''
                shell = subprocess.Popen(['bash', '-c', definitions + '\n' + stubs +
                                          'install_vm_boot_traps\n' + blocks[index]],
                                         env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                         start_new_session=True)
                try:
                    try:
                        stdout, stderr = shell.communicate(timeout=4)
                    except subprocess.TimeoutExpired as error:
                        self.assertIn(b'PASS: SUITE smoke DONE', error.stdout or b'')
                        self.fail(f'{platform} does not finish cleanup after scored DONE')
                    self.assertEqual(shell.returncode, 0, stdout + stderr)
                    guest.wait(timeout=2)
                    self.assertTrue(stopped.exists(), stdout + stderr)
                    self.assertEqual(record.read_text(), '0')
                    self.assertIn('SUITE smoke DONE', serial.read_text())
                finally:
                    try:
                        os.killpg(shell.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    shell.wait()
                    if guest.poll() is None:
                        guest.kill()
                    guest.wait()

    def test_all_monitors_record_stall_despite_census(self):
        for name in ('watch-qemu.py', 'x86-gate-boot.py', 'x86-suite-boot.py', 'watch-boot.py', 'wait-boot-done.py'):
            with self.subTest(monitor=name), tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp)
                user, kernel, stopped = (out / n for n in ('serial_user.log','serial_kernel.log','stopped'))
                replay = [sys.executable, '-c', REPLAY, str(user), str(kernel), str(stopped)]
                if name == 'watch-qemu.py':
                    # ARM64 mixes userspace and kernel lines on one UART.
                    replay = [sys.executable, '-c', REPLAY, str(user), str(user), str(stopped)]
                    args = ['--serial',str(user),'--mode','tests','--idle-exit','0','--'] + replay
                elif name == 'x86-gate-boot.py':
                    args = [str(kernel),str(user)] + replay
                elif name == 'x86-suite-boot.py':
                    args = [str(ROOT),str(out),'smoke','10'] + replay
                elif name == 'watch-boot.py':
                    args = ['--serial',str(user),'--serial',str(kernel),'--'] + replay
                else:
                    user.write_text('SIGKILL_TEARDOWN_TEST: FAIL\n')
                    args = [str(user),'tests','--timeout','10']
                result = subprocess.run([sys.executable,'-c',INJECT,str(ROOT/'scripts'/name)] + args,
                                        capture_output=True,text=True,timeout=6)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                record = json.loads(Path(str(user)+'.stall.json').read_text())
                self.assertEqual(record['status'],'stalled')
                self.assertEqual(record['last_progress_line'],'SIGKILL_TEARDOWN_TEST: FAIL')
                self.assertGreaterEqual(record['inactivity_seconds'],.5)
                self.assertIn('STALLED:',result.stdout)
                if name != 'wait-boot-done.py':
                    self.assertTrue(stopped.exists(), result.stdout + result.stderr)
                    self.assertIn('CENSUS',kernel.read_text() if kernel.exists() else user.read_text())
                if name == 'x86-suite-boot.py':
                    self.assertEqual(json.loads((out/'suite-results.json').read_text())[0]['verdict'],'STALLED')


if __name__ == '__main__':
    unittest.main()
