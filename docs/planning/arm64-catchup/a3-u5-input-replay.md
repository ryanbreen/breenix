# A.3 u5: input registration and replay receipt guard

Continued `arm-catchup/a-3` at `6508ab17`. Read the full ARM catch-up plan,
U1/U2/U16/U19/U28/U29/U31, G5/G7/G8, the debugging protocol and the applicable
repository instructions before deciding the unit's scope.

The retained u1 capture, Vigil `20261111T054423Z-qemu-34249`, has **no failing
input-core or input-devices cases**: all nine input-core cases and both
input-devices cases passed (`serials/a3-u1.txt:3-13`). There is consequently
no failing case to attribute to a producing line or repair in this unit.
The four baseline failures are performance/latency-under-animation,
recovery/restart-adopts, recovery/supervisor-restarts and
liveness/memory-bounded; this unit clears none of them.

The existing production path is:

- ARM keyboard registration and capabilities are at
  `kernel/src/drivers/virtio/input_mmio.rs:380-393`, tablet registration and
  capabilities at `:429-478`. Both bind the shared replay interface.
- `kernel/src/input/control.rs:63-72` selects the bound driver and writes
  calls, posted and ring to byte offsets 32, 36 and 40 in the receipt.
- Reset-custody replay starts at `input_mmio.rs:856`. It refuses an
  uninitialized device, resets before writing buffers, and sends replayed
  events through `complete_buffer` at `:571`. That same completion adapter
  reports the event and reposts its descriptor at `:665-671`.
- `input_mmio.rs:929-930` returns the observed report calls and posted count
  with the negotiated ring size. Report suppression is accounted by
  `kernel/src/test_control/input_replay.rs:43-50`; descriptor reposting does
  not depend on whether publication is suppressed.
- The suite client already enforces the receipt contract at
  `userspace/programs/src/wm_suite/input.rs:182-194`: reports are nonzero
  normally and zero when suppressed, ring is nonzero, and posted equals ring.

Under U29, test-only commit `e6106250` adds one regression guard,
`tests/input_replay_receipt_test.py`. It compiles the production client's
receipt validator verbatim into a temporary optimized host Rust test. Both
keyboard and pointer selectors, normally and suppressed, accept the required
receipt and reject wrong report counts, empty rings, missing descriptors and
excess descriptors. Nonzero reports are not restricted to a fixed count.
The guard checks receipt acceptance; it does not simulate DMA or establish
fresh device execution evidence. No runtime source was extracted or changed.

Validation: `CARGO_BUILD_JOBS=4 python3 tests/input_replay_receipt_test.py -v`
passed **1/1** on the existing implementation. Its Rust compilation uses
`taskpolicy -b`, optimization and `-Dwarnings` on this Mac; compilation and
execution succeeded. `git diff --check` passed. This is a passing regression
guard, not a red-first fix, as U29 requires when the behavior already exists.

Only the host test and this report change. No ARM or x86 runtime code changes,
so this unit reaches **no x86 suite** and needs no kernel or userspace release
rebuild. No boot, R240 operation, deployment, PR or merge was performed.
Final-head acceptance remains with the acceptance unit; cases and thresholds
are unchanged.
