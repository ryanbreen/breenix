# A.3 ARM reap-path continuation

The retained acceptance `20261111T154755Z-qemu-92251`, runtime `7dc1a45c`,
reports 80 passed and one failed case:

```text
SUITE window-manager CASE recovery/restart-adopts FAIL ms=3481 msg=reap owned process: no acknowledgment within 500 ms
```

This fails in `Child::stop` before replacement creation. The capture contains
PID 665's signal-exit line and ASID census, but no exit-phase timings. Those
lines precede zombie publication and cannot attribute the 500 ms interval.
The previous native result remains failed evidence, as recorded in
[a3-continuation.md](a3-continuation.md).

## Source defect and check-first repair

`sys_exit_aarch64` leaves the process TTBR0 and clears its return shadows before
calling `ProcessScheduler::handle_thread_exit`. The latter previously checked
whether a CPU still held the root. With no live root, it synchronously called
`release_process_leaves`, through `cleanup_cow_frames` to
`ProcessPageTable::release_mapped_leaves`, while holding PROCESS_MANAGER and before
`terminate_minimal` published the zombie. That walk audits the address space and
releases mapped frames without a per-pass budget. The compositor's exit could
therefore put address-space-sized work in front of its parent's wait result.
This is a code-derived latency defect; the old capture does not establish how
much time that particular walk consumed.

Check-first commit `c53289cc` adds `tests/arm_exit_deferral_test.py`. It compiles
the production receipt branch with `-Dwarnings`. A quiescent-root fixture with
32,768 modeled mapped leaves fails on the old branch: 32,768 leaves walked before
zombie publication, expected 0. The repair passes both quiescent and live-root
controls, preserving the leaf custody in one receipt and creating no second
receipt on repeated exit. This is an ordering/custody test, not a synthetic timing
measurement. It changes no check on main and needs no G2 correction.

ARM normal exit now hands current and superseded tables to the existing retirement
pipeline, whether or not the root is live. Its epoch, hardware, shadow, cached-root
and live-row proofs and bounded leaf/table drain are unchanged. The obsolete
forced-live injection and its guard are removed: the existing pairing fixture
now exercises the deferral directly, retaining its first/repeat and reclaim
assertions. x86 production exit behavior is unchanged.

Six ARM exit stages are retained in the existing bounded pipeline ring (17–22):
entry, windows released, before PM, zombie publication, after PM, GPU handles
released. Stage 20 binds the thread to its PID. A failed reap writes the kill
request, kill-return and failed-wait timestamps, process status and recent exit
stages through the existing evidence channel. Successful polls do not write these diagnostics.
The unchanged 500 ms assertion still decides the result. These diagnostics are
present in the production ARM build; they are not toggled between boots.

## Validation and acceptance scope

The new deterministic check fails before the repair and passes after it, one
behavior run per changed code state. The ARM production kernel and the scoped
suite-window-manager release build use four jobs under `taskpolicy -b`.
The kernel release build completes in 1m 46s with no compiler warnings or
errors; Cargo emits only G35(2)'s pinned-core future-incompatibility advisory.
The scoped suite release build completes in 29.27s without diagnostics.
The ARM FP/SIMD guard passes (0 forbidden loads/stores, 32 allowlisted).
The updated suite ELF is installed, the disposable ext2 disk is refreshed,
and the userspace manifest/source/disk check passes. No other suite runs.
`claim-lint --base HEAD` and `git diff --check` pass for the new hunks.

The new runtime branches and diagnostic producers are `cfg(aarch64)`; x86's
production receipt branch is unchanged. Removing the boot-test forcing guard
changes no production x86 code. No further R240 boot is required by coordinator
ledger line 154 for this correction. The prior native 80/81 is not a passing ARM
cell. The required new boot is one headless ARM64 HVF window-manager launch via
`boot-interactive.sh --mode suite --suite window-manager --gpu-gl --no-display`.
It must meet coordinator line 143's unchanged 81-case selection, memory, ordered
READY and ANGLE/Metal requirements. No acceptance result is claimed here yet.
