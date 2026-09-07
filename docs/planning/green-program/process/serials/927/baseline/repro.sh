set -Eeuo pipefail
cd /root/breenix-927a
source /root/.cargo/env
export BREENIX_GATE_TMP=/root/breenix-927a-tmp TMPDIR=/root/breenix-927a-tmp
printf 'REVISION='; git rev-parse HEAD
cargo build --release --features boot_tests --bin qemu-uefi
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release --features boot_tests --bin qemu-uefi > "$TMPDIR/repro-image.txt"
