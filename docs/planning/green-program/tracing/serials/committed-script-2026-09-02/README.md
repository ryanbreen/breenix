# Committed-script battery — 2026-09-02

Prove-slot follow-up to the round-1 review finding (0 of 2 unmodified-script
attempts reached `TRACE_VALIDATION:PASS` — one per branch, see
`docs/planning/green-program/tracing/serials/2026-09-02-prove-battery-README.md`)
that the round-1 evidence came from an uncommitted scratch copy of
`scripts/test_tracing_via_gdb.sh`, not the script as it stood committed on
either `main` or the fix branch. This directory holds output from the
**committed script, run unmodified**, on both sides of the fix.

## What ran, and how

11 of 11 runs recorded in this directory (10 fix-branch boots + 1 main boot)
used the one `x86_64-breenix.json` testing build (`cargo build --release
--features testing,external_test_bins --bin qemu-uefi`) plus `cargo run -p
xtask -- create-test-disk`, built once in a scratch git worktree of
`fix/tracing-symbol-base` at `e7d019c6` on beast (`breenix-x86` Incus
container). `BREENIX_RUST_FORK_LIBRARY` was pointed at the shared checkout's
`rust-fork-real/library` (that directory is untracked — not a submodule in
this repo — and had to be supplied out-of-band for a fresh worktree to build
userspace at all; nothing under `kernel/` was touched or substituted).

**`fix-branch-e7d019c6/boot01`–`boot10`**: the committed script
(`scripts/test_tracing_via_gdb.sh`, no local edits) run 10 times from that
worktree with `--arch x86_64 --out <dir>`, sequentially, one QEMU instance at
a time.

**`main-3d601400/boot01`**: same worktree layout, but a second git worktree
checked out at `main`'s `3d601400` (the commit the fix branch diverged from),
so its copy of `scripts/test_tracing_via_gdb.sh` is the pre-fix version —
hardcoded `KERNEL_BASE=0x10000000000`, single virtio-blk-pci device. Its
`target/` was populated by copying (not symlinking — a symlinked `target`
made GNU `find`'s traversal come up empty and `xargs`'s no-input fallback ran
`ls -t` on the worktree root instead, misidentifying `KERNEL_BIN` as the
literal path `target`; real copies avoid that) the same
`x86_64-unknown-none`, `release`, `ovmf`, and `test_binaries.img` artifacts
built from the fix-branch worktree above — legitimate because `git diff
--stat 3d601400 e7d019c6` reports 130 files changed, 0 of 130 under
`kernel/`, `src/`, `libs/`, or `userspace/` (130 of 130 are under `scripts/`
or `docs/`), so the kernel binary the main-branch script boots is
byte-identical to the one already built. Run once, as the task specified.

Each `bootNN/` directory holds the artifacts the script itself writes
(`serial.txt`, `qemu.log`, and — on the fix branch, once the settle window
completes — `kernel_base.txt`, `gdb_output.txt`, `validation.txt`) plus
`harness_stdout.txt`, a capture of the script's own top-level stdout/stderr
banner. Binary dumps (`trace_buffers.bin`, `trace_enabled.bin`) and the OVMF
firmware copies the script stages per-run were not preserved — they're
per-run scratch, not evidence, and were multiple MB each.

## Results

### Fix branch (`e7d019c6`), 10 of 10 boots

| boot | past disk loading | kernel base | TRACE_ENABLED | CPU0 events | validator | harness exit |
|------|---|---|---|---|---|---|
| 01 | yes | 0x10000000000 | 0x1 | 520 | PASS | PASS |
| 02 | yes | 0x10000000000 | 0x1 | 591 | PASS | PASS |
| 03 | yes | 0x10000000000 | 0x1 | 530 | PASS | PASS |
| 04 | yes | 0x10000000000 | 0x1 | 498 | PASS | PASS |
| 05 | yes | 0x10000000000 | 0x1 | 545 | PASS | PASS |
| 06 | yes | 0x10000000000 | 0x1 | 610 | PASS | PASS |
| 07 | yes | 0x10000000000 | 0x1 | 518 | PASS | PASS |
| 08 | yes | 0x10000000000 | 0x1 | 534 | PASS | PASS |
| 09 | yes | 0x10000000000 | 0x1 | 555 | PASS | PASS |
| 10 | yes | 0x10000000000 | 0x1 | 535 | PASS | PASS |

10 of 10 boots: got past the disk-loading panic that round 1 found blocking
every unmodified run; derived `0x10000000000` from serial (the same base
round 1 observed on 29 of its 30 uncommitted-script boots — round 1's single
non-observing boot was a UEFI death before the offset line printed, not a
different base); `TRACE_ENABLED` read as the plausible value `0x1`; the
Python validator and the harness's own exit status both read PASS. **0 of 10
boots observed `0x8000000000`** — consistent with round 1, not expected to
recur, and none did. Machine-readable: `fix-branch-e7d019c6/summary.tsv`.

### `main` (`3d601400`), 1 of 1 boot

The disk-loading panic round 1 found, reproduced from the actual committed
script (not a description of it):

```
KERNEL PANIC: panicked at kernel/src/userspace_test.rs:182:13:
╔══════════════════════════════════════════════════════════════╗
║  ❌ FATAL: DISK LOADING FAILED                               ║
╠══════════════════════════════════════════════════════════════╣
║  Binary: hello_time                                          ║
║  Error: No test disk found (index 1)                         ║
║                                                              ║
║  Disk loading is MANDATORY. There is NO fallback.           ║
║                                                              ║
║  Ensure QEMU is configured with test disk as second         ║
║  VirtIO device (index 1).                                   ║
╚══════════════════════════════════════════════════════════════╝
```
(full text: `main-3d601400/boot01/serial.txt`). The kernel panics inside the
15s settle window, before GDB ever attaches — `main-3d601400/boot01/` has no
`kernel_base.txt`, `gdb_output.txt`, or `validation.txt` because the script
never reaches those steps; the harness's own settle-window check catches the
dead QEMU process and the run ends at `exit 1` (`Error: QEMU exited during
the settle window`, `harness_stdout.txt`). This is the failure the fix
branch's added second `virtio-blk-pci` device removes — 0 of 1 main-branch
attempt got past disk loading, 10 of 10 fix-branch attempts did.
