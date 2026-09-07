set -Eeuo pipefail
cd /root/breenix-927a
source /root/.cargo/env
export BREENIX_GATE_TMP=/root/breenix-927a-tmp TMPDIR=/root/breenix-927a-tmp
run_gate() {
    local name="$1"
    shift
    {
        echo "REVISION=$(git rev-parse HEAD)"
        echo "COMMAND=$*"
        git status --short
        "$@"
    } 2>&1 | tee "$TMPDIR/$name.log"
    echo "GATE_COMPLETED=$name"
    if grep -Eq '^[[:space:]]*(warning|error)(\[|:)' "$TMPDIR/$name.log"; then
        echo "COMPILE_DIAGNOSTICS=$name"; return 1
    fi
}
export BREENIX_STRUCTURE_JOBS=1 BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS=600
echo 'ORACLE_PREFLIGHT_RETRY: jobs=1 per_suite_timeout=600; no guest boot in the first attempt'
run_gate oracle-x86 bash docker/qemu/run-blocking-io-oracle-gate.sh --arch x86_64
echo 'ORACLE_PROOF_PASS'
