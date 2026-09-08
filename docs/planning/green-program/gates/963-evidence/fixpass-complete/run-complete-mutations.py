"""Verify successful-report framing against reversible emitter mutations."""
import runpy
from pathlib import Path

# Rerun the four earlier external mutations, preserving their historical logs.
helpers = runpy.run_path(str(Path(__file__).with_name('run-mutations.py')))
run = helpers['run']
fixture = helpers['fixture']
gate = helpers['gate']
original = fixture.read_text()
start = original.index('    let Some(fields)', original.index('fn complete_report('))
end = original.index('\n}', start)
weak = original[:start] + '    out.starts_with("[QMP_DUMP:capture=complete:") && out.contains(":decoded_events=-:") && out.lines().count() == 1' + original[end:]
run('complete-old-predicate', fixture, weak, 'rejects_malformed_complete_reports')
source = gate.read_text()
wire = r'[QMP_DUMP:capture=complete:reason=-:core=%s:decoded_events=%s:dump_ms=%s]\n'
assert source.count(wire) == 1
for name, changed in [
    ('missing-bracket', wire.replace(']', '')),
    ('missing-reason', wire.replace('reason=-:', '')),
    ('wrong-reason', wire.replace('reason=-', 'reason=wrong')),
    ('nondigit-ms', wire.replace('dump_ms=%s', 'dump_ms=x%s')),
    ('leading-noise', 'noise' + wire),
    ('trailing-noise', wire + 'noise'),
    ('duplicate', wire + wire),
]:
    run('complete-emitter-' + name, gate, source.replace(wire, changed),
        'fake_qmp_dump_precedes_sigterm_even_when_decode_fails')
for name, before, after in [
    ('empty-core', '"$core" "${decoded:--}" "$dump_ms"', '"" "${decoded:--}" "$dump_ms"'),
    ('empty-ms', '"$core" "${decoded:--}" "$dump_ms"', '"$core" "${decoded:--}" ""'),
    ('empty-decoded', '"$core" "${decoded:--}" "$dump_ms"', '"$core" "" "$dump_ms"'),
    ('nondigit-decoded', '"$core" "${decoded:--}" "$dump_ms"', '"$core" "oops" "$dump_ms"'),
]:
    assert source.count(before) == 1
    run('complete-emitter-' + name, gate, source.replace(before, after),
        'fake_qmp_dump_precedes_sigterm_even_when_decode_fails')
assert gate.read_text() == source
assert fixture.read_text() == original
print('All complete-report mutations restored.')
