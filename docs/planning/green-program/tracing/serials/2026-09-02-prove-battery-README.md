# Tracing KERNEL_BASE fix — evidence battery, 2026-09-02

Branch: `fix/tracing-symbol-base` @ `8612e830` (not pushed, not merged), on top of
`main` @ `3d601400`. Beast container: `breenix-x86`.

## What this battery does and does not establish

This battery independently re-checks the specific behavior under test: that
`scripts/test_tracing_via_gdb.sh` on `fix/tracing-symbol-base` derives
`KERNEL_BASE` for x86_64 from each boot's own
`virtual_address_offset: 0x...` serial line instead of a hardcoded constant.
It does not by itself turn the Tracing-x86 or Bus-x86 cells green, and it
does not certify the harness as a whole — see the disclosed defect below,
which is orthogonal to the fix and was found, not fixed, in this round.

## Disclosed defect found during this round (pre-existing on `main`, unrelated to the KERNEL_BASE fix)

`scripts/test_tracing_via_gdb.sh`'s x86_64 QEMU invocation (both on `main`
and unchanged by this fix's diff) attaches only one virtio-blk-pci device
(the UEFI boot disk). A kernel built exactly as the script's own error
message instructs — `cargo build --release --features testing,external_test_bins
--bin qemu-uefi` — unconditionally requires a *second* virtio-blk-pci device
carrying `target/test_binaries.img` (see `kernel/src/userspace_test.rs`,
`get_test_binary()`, gated on `feature = "testing"` alone, not on
`external_test_bins`); without it every boot panics with `FATAL: DISK LOADING
FAILED` before the harness's settle window completes. This reproduced
identically on both `fix/tracing-symbol-base` and plain `main` in a
from-scratch build in this round: 0 of 2 unmodified-script attempts (one per
branch) reached `TRACE_VALIDATION:PASS`; both hit the disk panic. `git show
main:scripts/test_tracing_via_gdb.sh` shows the missing device is identical on
both branches — this fix's diff never touches that QEMU invocation block.

This is a second, previously undocumented gap, separate from the KERNEL_BASE
defect this round re-checked. It was not fixed in the round that produced the
30-boot evidence below — that round's scope was the KERNEL_BASE fix only ("Do
NOT touch kernel/ - this is a test-harness change only" / single-slot / no
unrelated changes). A follow-on correction round has since added the missing
`testdisk` device directly to the committed script (see that commit's
message); the 30 boots recorded in this directory (20 fix-branch + 10
old-bytes, below) predate that fix and were gathered the way the next section
describes, not by running the now-repaired committed script.
<!-- claim-lint:ok: 0 of 2 unmodified-script attempts (one per branch) reached
     TRACE_VALIDATION:PASS in this round, both blocked on the same disk-device
     gap; see the "How evidence ... was actually gathered" section below for
     the workaround used to still exercise the KERNEL_BASE lines under test. -->

## How evidence in this directory was actually gathered

**Which script each boot in this directory ran, stated plainly:** at the time
this 30-boot battery was gathered, the committed `scripts/test_tracing_via_gdb.sh`
had not completed a single boot in a from-scratch build on either branch (0 of
2 unmodified-script attempts, as above) — the missing `testdisk` device made
every unmodified run panic with `FATAL: DISK LOADING FAILED` before the settle
window. None of the 30 boots below ran that (at-the-time-broken) committed
script. To still gather per-boot evidence about the KERNEL_BASE derivation
logic specifically, each of the 30 boots instead ran a local, uncommitted
scratch copy of the harness (`scripts/test_tracing_via_gdb.LOCAL_ONLY.sh`,
created for that round only, deleted afterward, never committed) with exactly
one addition: the missing `testdisk` virtio-blk-pci device, wired identically
to the existing pattern in `docker/qemu/run-boot-parallel.sh` (lines 88-90
there). The KERNEL_BASE-derivation lines exercised are otherwise
byte-identical to the committed script as it stood at that time — the
derivation block on `fix/tracing-symbol-base`, and the hardcoded-constant
block on `main` for the old-bytes comparison below — so the addition changed
what a boot could reach, not what KERNEL_BASE logic ran. A later correction
round folded that same device addition into the committed script itself, so a
fresh run of `scripts/test_tracing_via_gdb.sh` no longer needs the scratch
copy — but that later fix produced no new boots of its own and is not
represented in the evidence below.

## Fix-branch battery (`fix/tracing-symbol-base` @ `8612e830`)

20 boots run, `fix-branch-2026-09-02/summary.tsv` is the one-line-per-boot
index. Per-boot files: `bootNN-harness_stdout.txt` (full script stdout and
stderr), `bootNN-serial.txt` (raw guest serial capture), `bootNN-gdb_output.txt`,
`bootNN-validation.txt`, `bootNN-kernel_base.txt` (the script's own
derived-base line) where the boot reached that stage.

- 17 of 20 boots reached the point of deriving `KERNEL_BASE` from serial. The
  other 3 (boot04, boot06, boot10) had their QEMU process killed by an
  external signal during the settle window — `bootNN-harness_stdout.txt` for
  each of the 3 shows `Killed`, and the harness's pre-existing
  `kill -0 "$QEMU_PID"` liveness check
  (`scripts/test_tracing_via_gdb.sh:236-240`) exits with `Error: QEMU exited
  during the settle window` before control ever reaches the derivation block
  at :247. None of the 3 reached that block — no `bootNN-kernel_base.txt` was
  written for any of them — so none of them ran the code under test. That is
  not because no base line existed yet on their serial: boot06 and boot10
  both printed `virtual_address_offset: 0x10000000000` at line 16 of their
  own `bootNN-serial.txt` before the external kill landed, and the liveness
  check simply exited before the harness ever read it. Only boot04's serial
  is silent on the offset: 2 lines, ending in `BdsDxe: starting Boot0002`,
  dead in UEFI before the bootloader printed anything. Most likely resource
  contention with an unrelated, concurrently running boot loop in the same
  shared container (see the note below). Not a KERNEL_BASE-derivation failure
  either way — the liveness check that stopped these 3 predates this fix and
  sits above the derivation block it gates.
- Of the 17 that reached derivation, 17 of 17 derived `KERNEL_BASE =
  0x10000000000`; 0 of 17 derived `0x8000000000` (per each boot's own
  `bootNN-kernel_base.txt`).
- Of the 17, TRACE_ENABLED read as a plausible boolean (`0x1`) in 14 of 17; a
  legitimate `0x0` (a real atomic-load value, not instruction-byte garbage —
  the base derivation was not in question in either case) in 2 of 17
  (boot16, boot18 — see `bootNN-gdb_output.txt` for each: GDB's PC read
  `0x100000cdffe` and `TRACE_CPU0_WRITE_IDX = 0` in both, consistent with the
  guest still being early in `kernel_main`'s init sequence, before
  `tracing::enable()`, when GDB attached at the end of the fixed 15s
  wall-clock settle window); and missing from `gdb_output.txt` entirely in 1
  of 17 (boot12 — see boot12-gdb_output.txt: GDB's own session errored with
  `Remote replied unexpectedly to 'vMustReplyEmpty'` after connecting, a
  gdbstub remote-protocol error, not a wrong-base symptom — boot12's own
  `kernel_base.txt` still shows the base was derived correctly).
- Of the 14 of 17 with a plausible `TRACE_ENABLED = 0x1`, 13 of 14 produced
  `TRACING_EVIDENCE:x86_64:PASS` and 1 of 14 (boot17) produced
  `TRACING_EVIDENCE:x86_64:FAIL`: per boot17-gdb_output.txt, TRACE_ENABLED
  read `0x1` correctly and GDB's PC read `0x10000364d85`, a valid kernel-text
  address for the derived base, but `TRACE_CPU0_WRITE_IDX = 0` — no events
  had been recorded yet, so `trace_memory_dump.py --validate` correctly
  failed the empty dump (see boot17-validation.txt). Not a base-derivation
  failure either.
- boot12, boot16, boot17 and boot18's thin or errored dumps, together with
  the 3 externally-killed boots, are consistent with one explanation: `ps
  aux` inside the container during this battery showed an unrelated,
  concurrently running `#693` KVM-accelerated boot loop competing for the
  same 8 host CPUs (not captured to a file in this directory — a live
  observation during the run, offered as a plausible explanation rather than
  an established cause), and this harness's x86_64 path runs under TCG
  (software emulation, no KVM), which can fall well behind wall-clock time
  under host contention — so a fixed 15s settle window does not assure any
  particular amount of guest-side progress. This is an offered explanation
  for the battery's environment, not a defect in the KERNEL_BASE fix under
  test, and it is not independently confirmed.
- 13 of 20 total boots in this battery produced `TRACING_EVIDENCE:x86_64:PASS`.

## Old-bytes battery (plain `main` @ `3d601400`, hardcoded `KERNEL_BASE=0x10000000000`)

10 boots run, `old-bytes-2026-09-02/summary.tsv` is the index. Same local
testdisk-only patch as above; the KERNEL_BASE derivation logic itself is
untouched (still the hardcoded constant on this branch). Each boot's summary
line also records `actual_offset`, the real `virtual_address_offset:` value
read directly off that boot's own `bootNN-serial.txt` — the same source the
fix reads, used here purely as an independent check, not fed into this
old-bytes harness.

- 10 of 10 boots' actual serial-printed offset (per each boot's own
  `bootNN-serial.txt`) was `0x10000000000` — identical to the hardcoded
  constant.
- `oldBytesFailureShown = false`. The task asked this battery to demonstrate
  the failure the fix removes by landing a boot on `0x8000000000` at the old,
  hardcoded-base bytes; that did not happen within the allotted 10 boots (0
  of 10) because the alternate base was not observed at all in this round, on
  either branch, across 30 boots total (20 in the fix-branch battery above +
  10 here). This report says so rather than asserting a failure that was not
  witnessed.

## What this does and does not say about the `0x8000000000` claim

The defect description this round re-checked states the bootloader "has been
observed to pick" `0x8000000000` on some boots and `0x10000000000` on others,
citing `breenix-gdb-chat/scripts/gdb_chat.py`'s header comment and CLAUDE.md's
GDB section. This round does not contradict that claim; it simply did not
reproduce it: of the 30 boots run in this round (both batteries, above), on
identical QEMU/OVMF versions, identical `-m 512 -smp 1` fixed memory/CPU
config, and (for the fix-branch battery) an identical binary, 29 of 30 landed
at `0x10000000000` (per each boot's own serial-printed
`virtual_address_offset:` line) and the remaining 1 (fix-branch boot04)
attests nothing — its serial is 2 lines, dead in UEFI before the bootloader
ran, with no offset line to read. 0 of 30 attested `0x8000000000`. The base a
UEFI bootloader picks depends on the memory map
the firmware hands it, which this harness's fixed `-m 512` may make
effectively deterministic in this one environment even where it is not
deterministic in general — different RAM sizes, different OVMF builds, or a
live/interactive boot configuration could plausibly still exercise
`0x8000000000`, per the CLAUDE.md GDB section's own boot-to-boot account, but
that is offered as a plausible explanation, not something this round
independently checked. The KERNEL_BASE fix reads the base from each boot's
own serial output regardless of which value it turns out to be, so it does
not depend on this round having observed the alternate value to be correct by
construction — but this round witnessed that alternate value in 0 of the 30
boots run, and says so plainly rather than asserting a failure case that did
not occur here.
