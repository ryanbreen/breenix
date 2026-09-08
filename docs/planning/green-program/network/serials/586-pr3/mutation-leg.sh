set -eu
mode=$1
export TMPDIR="$PWD/.tmp" BREENIX_GATE_TMP="$PWD/.gate-tmp" BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library
export __CARGO_TESTS_ONLY_SRC_ROOT="$PWD/.tmp/rust-src/library"
out="$PWD/docs/planning/green-program/network/serials/586-pr3/$mode"
mkdir -p "$out"
python3 .tmp/mutate.py "$mode"
trap 'python3 .tmp/mutate.py restore' EXIT
{
 git rev-parse HEAD
 git diff -- kernel/src/net/tcp.rs kernel/src/test_framework/registry.rs
 cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
} > "$out/build.txt" 2>&1
if rg -q '^warning|^error' "$out/build.txt"; then echo 'build diagnostics: stopped'; exit 1; fi
if [ "$mode" = always-extend ]; then
 { git rev-parse HEAD; bash scripts/run-structure-tests.sh loopback_pump_structure loopback_guest_budget; echo "ratchet_exit=$?"; } > "$out/ratchet.txt" 2>&1 || true
fi
RAW_BOOT_SECONDS=30 python3 .tmp/raw-boot.py "$mode"
python3 .tmp/mutate.py restore
{ git rev-parse HEAD; BREENIX_STRICT_SCORE_ONLY="$out/serial.txt" bash docker/qemu/run-aarch64-boot-test-strict.sh 1; } > "$out/score.txt" 2>&1 || echo 'score_exit=1' >> "$out/score.txt"
