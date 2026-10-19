#!/usr/bin/env python3
"""Exercise gate retry control flow without launching a VM."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
arm = (ROOT / 'docker/qemu/run-aarch64-boot-test-strict.sh').read_text()
loop = arm[arm.index('i=1\nattempt=0\n'):arm.index('\nEND_TIME=$(date +%s)')]
for replacement, expected in [(0, '1 0 1 2'), (1, '0 1 1 2')]:
    script = f'''set -euo pipefail
ITERATIONS=1 SUCCESSES=0 FAILURES=0 STARVATIONS=0
STARVED_ITERATIONS= FAILED_ITERATIONS=
run_single_test() {{ if [ "$attempt" -eq 1 ]; then return 2; else return {replacement}; fi; }}
{loop}
echo "$SUCCESSES $FAILURES $STARVATIONS $attempt"
'''
    result = subprocess.run(['bash', '-c', script], text=True, capture_output=True, check=True)
    assert result.stdout.splitlines()[-1] == expected, result.stdout

x86 = (ROOT / 'docker/qemu/run-x86-gate.sh').read_text()
start = x86.index('  if [ "$census_ok" = true ] && [ "$verdict_ok" = true ]; then')
finish = x86.index('\ndone\nTOTAL_SECS=', start)
body = x86[start:finish]
with tempfile.TemporaryDirectory(prefix='breenix-starvation-retry-') as tmp:
    for census, expected in [('true', '1 0 2'), ('false', '0 1 1')]:
        script = f'''set -euo pipefail
OUTDIR="$1"
i=1 attempt=0 PASS=0 FAIL=0
INSPECTOR_START_MS=1000
BREENIX_RUNS_GATE_ARGV=()
host_slot_release() {{ :; }}
breenix_runs_import_nonfatal() {{ echo "IMPORTED $4 $5"; }}
while [ "$i" -le 1 ]; do
  attempt=$((attempt+1))
  census_ok={census} census_reason=census pci_census_line=devices
  scoring_deadline_missed=false boot_completed=true utc_clock_failed=false
  verdict_ok=false verdict_status=2 verdict_reason=STARVED
  if [ "$attempt" -gt 1 ]; then verdict_ok=true; verdict_status=0; fi
{body}
done
echo "$PASS $FAIL $attempt"
'''
        result = subprocess.run(['bash', '-c', script, 'test', tmp], text=True, capture_output=True, check=True)
        assert result.stdout.splitlines()[-1] == expected, result.stdout
        if census == 'true':
            assert 'IMPORTED STARVED 2' in result.stdout
            assert 'IMPORTED PASS 0' in result.stdout
        else:
            assert 'IMPORTED FAIL 1' in result.stdout
            assert 'IMPORTED STARVED' not in result.stdout
print('starvation retry control flow: ok')
