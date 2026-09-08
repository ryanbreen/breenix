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
run_gate boot-tests-only bash docker/qemu/run-x86-boot-tests-only.sh
run_gate x86-boot-tests bash docker/qemu/run-x86-boot-tests.sh
run_gate x86-prod bash docker/qemu/run-x86-prod-profile-boot-test.sh
run_gate oracle-x86 bash docker/qemu/run-blocking-io-oracle-gate.sh --arch x86_64
echo 'PROOF_BATTERY_PASS'
