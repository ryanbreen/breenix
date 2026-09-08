#!/usr/bin/env python3
"""Exercise strict-gate shell status propagation without launching QEMU."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
GATE = ROOT / "docker/qemu/run-aarch64-boot-test-strict.sh"
source = GATE.read_text()


def run(script, env):
    return subprocess.run(["bash", "-c", script], env=env, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT)


with tempfile.TemporaryDirectory(prefix="softirq-status-") as tmp:
    env = dict(os.environ, BREENIX_GATE_TMP=tmp, OUTPUT_DIR=tmp)
    serial = Path(tmp) / "serial.txt"
    baseline = (ROOT / "tests/fixtures/udp-socket-lock-aarch64-serial.txt").read_text()
    serial.write_text(baseline)
    result = subprocess.run(["bash", str(GATE), "1"],
                            env=dict(env, BREENIX_STRICT_SCORE_ONLY=str(serial)),
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    assert result.returncode == 0, result.stdout
    # Synthetic scorer legs, not guest receipts: retain the other required
    # oracles so the softirq status is the property under test.
    baseline, removed = re.subn(r"\[SOFTIRQ_DEFERRAL_ORACLE:[^\]]*\]", "", baseline)
    assert removed == 1, "expected one recorded softirq oracle"
    oracle = "[SOFTIRQ_DEFERRAL_ORACLE:arch=aarch64:cpu=1:budget_ticks=250:wait_ticks=2:wait_ns=15000000000:dispatches=0:iterations=10:verdict=starved]"
    for contents, status, label in [(oracle, 2, "INCONCLUSIVE"),
                                    (oracle.replace("15000000000", "0"), 1, "FAIL"),
                                    (oracle.replace("starved", "lost"), 1, "FAIL"),
                                    (oracle + "\nKERNEL PANIC", 1, "FAIL"),
                                    (oracle + "\n[BOOT_TESTS:FAIL]", 1, "FAIL")]:
        serial.write_text(baseline + "\n" + contents + "\n")
        result = subprocess.run(["bash", str(GATE), "1"],
                                env=dict(env, BREENIX_STRICT_SCORE_ONLY=str(serial)),
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        assert result.returncode == status, result.stdout
        assert f"SCORE: {label}" in result.stdout, result.stdout

    # Execute the actual frozen-snapshot verdict/report/import tail. Only the
    # scorer and capture metadata providers are stubbed; no QEMU is required.
    tail = source.split("    local FAIL_DETAIL\n", 1)[1].split('\necho "========================================="', 1)[0]
    report = source.split("report_failure() {", 1)[1].split("\nrun_single_test() {", 1)[0]
    for status, label in [(0, "PASS"), (1, "FAIL"), (2, "INCONCLUSIVE")]:
        script = '''set -e
iteration=1
ENDED_BY_LOOP=early_pass
QEMU_STILL_ALIVE=0
HOST_MS_START=0
HOST_MS_END=1
DEADLINE_SERIAL="$OUTPUT_DIR/deadline.txt"
BREENIX_RUNS_GATE_ARGV=(test)
gcd_classify_report() { echo capture; }
gbf_last_heartbeat_uptime_ms() { echo 0; }
gbf_emit_line() { echo facts; }
breenix_runs_import_nonfatal() { echo "IMPORT $4 $5"; }
score_serial() { echo oracle; return STATUS; }
report_failure() {REPORT
run_tail() {
    local FAIL_DETAIL
@@TAIL@@
if run_tail; then exit 0; else exit $?; fi
'''.replace("STATUS", str(status)).replace("REPORT", report).replace("@@TAIL@@", tail)
        result = run(script, env)
        assert result.returncode == status, result.stdout
        assert f"IMPORT {label} {status}" in result.stdout, result.stdout
        if status == 2:
            assert "[INCONCLUSIVE]" in result.stdout, result.stdout
            assert "[FAIL]" not in result.stdout, result.stdout

    # Run the real aggregation for mixed iterations; a kernel failure wins
    # over starvation; starvation increments the inconclusive count.
    summary = source.split("START_TIME=$(date +%s)\n", 1)[1]
    for statuses, expected in [("0 0", 0), ("0 2", 2), ("2 2", 2), ("2 1", 1), ("1 2", 1)]:
        script = '''set -e
START_TIME=$(date +%s)
ITERATIONS=2
SUCCESSES=0
FAILURES=0
STARVATIONS=0
FAILED_ITERATIONS=""
STARVED_ITERATIONS=""
statuses=(STATUSES)
run_single_test() { return "${statuses[$1-1]}"; }
'''.replace("STATUSES", statuses) + summary
        result = run(script, env)
        assert result.returncode == expected, result.stdout
    print("strict status: score-only, frozen boot import/report, mixed aggregation PASS")
