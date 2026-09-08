"""Run reversible fixture/deadline mutations through the structure runner."""
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[6]
os.chdir(root)
os.environ.update(TMPDIR=str(root / '.tmp'), BREENIX_GATE_TMP=str(root / '.gate-tmp'))
os.environ['PATH'] = '/opt/homebrew/bin:' + os.environ['PATH']
evidence = Path(__file__).resolve().parent
fixture = Path('tests/gate_qmp_backstop_structure.rs')
gate = Path('docker/qemu/lib/gate-qmp-backstop.sh')
original = fixture.read_text()
revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()

def run(name, path, changed, test):
    saved = path.read_text()
    assert changed != saved
    try:
        path.write_text(changed)
        result = subprocess.run(['bash', 'scripts/run-structure-tests.sh', 'gate_qmp_backstop_structure', test], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        (evidence / (name + '.log')).write_text(f'Revision: {revision}; working fixture patch plus named mutation {name}\n{result.stdout}\nexit={result.returncode}\n')
        assert result.returncode == 101, (name, result.returncode, result.stdout)
        assert 'test result: FAILED' in result.stdout
        assert 'warning:' not in result.stdout
        print(f'{name}: expected red exit {result.returncode}', flush=True)
    finally:
        path.write_text(saved)

old_adapter = original.replace("            pathlib.Path('bounded').write_text(str(budget))\n        else:\n            raise AssertionError('hung dump unexpectedly completed')", "        assert result == 124, 'hung dump unexpectedly completed'\n        pathlib.Path('bounded').write_text(str(budget))")
run('v1-old-adapter', fixture, old_adapter, 'timeout_marker_requires_expiry_not_early_exit_124')
start = original.index('    let prefix =', original.index('fn partial_report('))
end = original.index('\n}', start)
old_report = original[:start] + '    out.contains(&format!("capture=partial:reason={reason}:")) && out.lines().count() == 1' + original[end:]
for name in ['missing_socket', 'hung_dump']:
    run('v2-old-' + name, fixture, old_report, name + '_report_rejects_malformed_lines')
run('shipped-deadline-removed', gate, gate.read_text().replace('timeout --kill-after=1 "$budget_s" bash', 'bash'), 'shipped_capture_deadline_and_mutations')
assert fixture.read_text() == original
print('Source mutations restored.')
