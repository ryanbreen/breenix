"""Exercise the production reserve expression and collector protocol on the host."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def run_rust(test, code):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory)
        (path / 'check.rs').write_text(code)
        command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
        if sys.platform == 'darwin':
            command = ['taskpolicy', '-b', *command]
        result = subprocess.run(command, capture_output=True, text=True)
        test.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
        test.assertEqual(result.returncode, 0, result.stdout + result.stderr)


class Collection(unittest.TestCase):
    def test_reserve_is_only_for_collecting_sessions(self):
        rel = 'libs/libbreenix/src/suite.rs'
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        expression = next(line.strip() for line in source.splitlines() if line.strip().startswith('let logic_limit_ms ='))
        run_rust(self, r'''
struct Case { collects:bool }
fn limit(session:Option<()>,case:&Case,limit_ms:u64)->u64 {
// The historical expression ignores this input.
let _ = case.collects;
EXPRESSION
logic_limit_ms
}
#[test]fn only_collection_consumes_the_reserve(){
    for session in [None,Some(())]{for collects in [false,true]{
        assert_eq!(limit(session,&Case{collects},3000),if session.is_some() && collects {1500}else{3000});
    }}
    assert_eq!(limit(Some(()),&Case{collects:true},1000),0);
}
'''.replace('EXPRESSION', expression))

    def test_handshake_states_and_deadlines(self):
        source = (ROOT / 'tests/suite_collection.rs').read_text().replace('../libs/', str(ROOT / 'libs') + '/')
        run_rust(self, source)


if __name__ == '__main__':
    unittest.main()
