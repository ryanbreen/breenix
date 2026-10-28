"""CE recovery errors stay visible to the scorer and let console work proceed."""
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class RecoveryContinuation(unittest.TestCase):
    def test_recorded_or_io_failure_returns_without_a_userspace_panic(self):
        source=(ROOT/'userspace/programs/src/gt710_wait_probe.rs').read_text()
        tail=source[source.index('    let recovery='):source.rindex('}')]
        tail=tail.replace('std::fs::read_to_string("/proc/gt710-recovery")','read_recovery()')
        harness=r'''
fn read_recovery()->std::io::Result<String> {
    if std::env::args().nth(1).unwrap()=="io" { return Err(std::io::Error::from(std::io::ErrorKind::Other)); }
    Ok(std::env::args().nth(1).unwrap())
}
fn probe() {
TAIL
}
fn main() {probe();println!("CONSOLE_CONTINUES");}
'''.replace('TAIL',tail)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory);(path/'probe.rs').write_text(harness)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(path/'probe.rs'),'-o',str(path/'probe')],check=True,cwd=ROOT)
            for value in ['GT710_RECOVERY_ERROR CE completion failed; backing retained\n','io','GT710_RECOVERY {"schema":1}\n']:
                result=subprocess.run([str(path/'probe'),value],capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertTrue(result.stdout.endswith('CONSOLE_CONTINUES\n'))
                self.assertIn('GT710_RECOVERY_ERROR' if value=='io' else value.strip(),result.stdout)
                if value!='GT710_RECOVERY {"schema":1}\n':
                    api=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
                    with self.assertRaises(ValueError):api['recovery_gate'](result.stdout)

if __name__=='__main__':unittest.main()
