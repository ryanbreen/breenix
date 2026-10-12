"""Exercise production monitor exits and cleanup with synthetic serials."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
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
