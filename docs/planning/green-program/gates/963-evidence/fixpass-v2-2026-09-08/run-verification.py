"""Record reversible old-predicate mutations and restored structure suites."""
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[6]
os.chdir(root)
os.environ.update(TMPDIR=str(root / '.tmp'), BREENIX_GATE_TMP=str(root / '.gate-tmp'))
os.environ['PATH'] = '/opt/homebrew/bin:' + os.environ['PATH']
evidence = Path(__file__).resolve().parent
fixture = Path('tests/gate_qmp_backstop_structure.rs')
original = fixture.read_text()


def run(name, command, expected, scope):
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    record = f'Revision: {revision}\nScope: {scope}\nCommand: {" ".join(command)}\n{result.stdout}\nexit={result.returncode}\n'
    (evidence / (name + '.log')).write_text('\n'.join(line.rstrip() for line in record.splitlines()).rstrip() + '\n')
    print(f'{name}: exit {result.returncode} (expected {expected})', flush=True)
    assert result.returncode == expected, result.stdout
    assert not any(line.startswith(('warning:', 'error:')) for line in result.stdout.splitlines()), result.stdout
    return revision


# Substitute each historical call-site predicate into the shared validator for
# its own reason, so the new regression directly tests what that predicate accepts.
for name, reason, predicate, test in [
    ('invalid-budget', 'invalid_budget', 'out.contains("reason=invalid_budget:")', 'invalid_budget_report_rejects_malformed_lines'),
    ('missing-tool', 'qmp_tool_missing', 'out.starts_with("[QMP_DUMP:capture=partial:reason=qmp_tool_missing:core=-:decoded_events=-:dump_ms=") && out.lines().count() == 1', 'missing_tool_report_rejects_malformed_lines'),
]:
    try:
        needle = 'fn partial_report(out: &str, reason: &str) -> bool {\n'
        mutated = original.replace(needle, needle + f'    if reason == "{reason}" {{ return {predicate}; }}\n')
        assert mutated != original
        fixture.write_text(mutated)
        run(name + '-old-predicate-red', ['bash', 'scripts/run-structure-tests.sh', 'gate_qmp_backstop_structure', test], 101, 'working fixture patch with historical ' + name + ' predicate')
    finally:
        fixture.write_text(original)
    run(name + '-restored-green', ['bash', 'scripts/run-structure-tests.sh', 'gate_qmp_backstop_structure', test], 0, 'restored working fixture patch')

assert fixture.read_text() == original
run('qmp', ['bash', 'scripts/run-structure-tests.sh', 'gate_qmp_backstop_structure'], 0, 'working fixture patch; source mutations restored')
before = set((root / '.gate-tmp').glob('breenix_gate_structure_preflight.*'))
revision = run('structure', ['bash', 'scripts/run-structure-tests.sh'], 0, 'working fixture patch; source mutations restored')
after = set((root / '.gate-tmp').glob('breenix_gate_structure_preflight.*'))
created = after - before
assert len(created) == 1, created
log_dir = created.pop()
with (evidence / 'structure-details.log').open('w') as output:
    output.write(f'Revision: {revision}\nScope: working fixture patch; source mutations restored\nCommand: bash scripts/run-structure-tests.sh (per-suite transcripts)\n')
    for stem in (log_dir / 'stems').read_text().splitlines():
        output.write(f'\nSuite source: tests/{stem}.rs\nRevision: {revision}\n')
        output.write((log_dir / (stem + '.metrics')).read_text())
        contents = (log_dir / (stem + '.log')).read_text()
        output.write('\n'.join(line.rstrip() for line in contents.splitlines()).rstrip() + '\n')
        assert not any(line.startswith(('warning:', 'error:')) for line in contents.splitlines()), stem
print('Restoration verified; suite detail transcripts saved.', flush=True)
