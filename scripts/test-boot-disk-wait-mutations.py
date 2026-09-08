#!/usr/bin/env python3
"""Run 508 source ratchets sequentially; restore each source in finally.

Invoke from the repository root with lane-local TMPDIR and BREENIX_GATE_TMP.
The unmodified 48610ba1 counter is the review's V-1 regression specimen.
"""
from pathlib import Path
import os
import subprocess

root = Path.cwd()
log_dir = Path(os.environ.get('TMPDIR', root / '.tmp'))
log_dir.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(log_dir), BREENIX_GATE_TMP=os.environ.get('BREENIX_GATE_TMP', str(root / '.gate-tmp')))
files = ['kernel/src/interrupts/context_switch.rs', 'kernel/src/per_cpu.rs', 'kernel/src/task/completion.rs', 'kernel/src/main.rs']
original = {f: Path(f).read_text() for f in files}
switch = files[0]
per = files[1]
completion = files[2]
main = files[3]
cases = [
    ('V1-original-selected-counter', switch, lambda s: subprocess.check_output(['git', 'show', '48610ba1:' + switch], text=True), 'departure'),
    ('V1-ignore-rollback', switch, lambda s: s.replace('dispatched_tid != old_thread_id', 'dispatched_tid == old_thread_id'), 'departure'),
    ('V2-switch-serial', switch, lambda s: s.replace('    // Update per-CPU current thread and TSS.RSP0', '    raw_serial_str("[SW]");\n    // Update per-CPU current thread and TSS.RSP0', 1), 'switch_and_admission'),
    ('V2-admission-log', per, lambda s: s.replace('pub fn can_schedule(saved_cs: u64) -> bool {', 'pub fn can_schedule(saved_cs: u64) -> bool {\n    log::warn!("probe");'), 'switch_and_admission'),
    ('V2-admission-port', per, lambda s: s.replace('pub fn can_schedule(saved_cs: u64) -> bool {', "pub fn can_schedule(saved_cs: u64) -> bool {\n    port.write(b'p');"), 'switch_and_admission'),
    ('wait-busy-spin', completion, lambda s: s.replace('enable_and_hlt()', 'spin_loop()'), 'boot_completion'),
    ('admission-remove-count', per, lambda s: s.replace('if !returning_to_userspace && current_preempt & 0x0fff_ffff != 0', 'if false'), 'kernel_schedule_admission'),
    ('final-marker-removed', main, lambda s: s.replace('disk_wait_oracle::tests_completed()', 'disk_wait_oracle::report()'), 'oracle_measures'),
]
for name, path, mutate, test_filter in cases:
    try:
        mutant = mutate(original[path])
        assert mutant != original[path], name
        Path(path).write_text(mutant)
        proc = subprocess.run(['bash', 'scripts/run-structure-tests.sh', 'boot_disk_wait_structure', test_filter], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        (log_dir / f'mutation-{name}.log').write_text(proc.stdout)
        print(f'{name}: exit {proc.returncode}', flush=True)
        assert proc.returncode == 101, (name, proc.stdout)
    finally:
        Path(path).write_text(original[path])
for path, source in original.items():
    assert Path(path).read_text() == source
print('8/8 mutants rejected; 4/4 source files restored')
