set -eu
export TMPDIR="$PWD/.tmp" BREENIX_GATE_TMP="$PWD/.gate-tmp"
export BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library
export __CARGO_TESTS_ONLY_SRC_ROOT="$PWD/.tmp/rust-src/library"
receipts="$PWD/docs/planning/green-program/network/serials/586-pr3"
mode="$1"
if [ "$mode" = r232-extension-deleted ]; then python3 .tmp/mutate.py extension-deleted; fi
trap 'python3 .tmp/mutate.py restore' EXIT
mkdir -p "$receipts/$mode"
{ git rev-parse HEAD; git diff -- kernel; cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64; } > "$receipts/$mode/build.txt" 2>&1
if rg -q '^warning|^error' "$receipts/$mode/build.txt"; then exit 1; fi
git diff -- kernel > "$receipts/$mode/source.patch"
shasum -a 256 target/aarch64-breenix-kernel/release/kernel-aarch64 > "$receipts/$mode/kernel.sha256"
python3 .tmp/mutate.py restore
set +e
{ git rev-parse HEAD; BREENIX_STARVED_OUTPUT="$receipts/$mode/loop" bash docker/qemu/run-aarch64-starved-loop.sh "$2" "$3"; echo exit=$?; } > "$receipts/$mode/loop.txt" 2>&1
