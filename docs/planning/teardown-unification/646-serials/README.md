# #646 — `kernel_stack_ownership_oracle` slot alloc/free delta equality is racy

Preserved while gating the #580 branch, per the standing rule that a failing
serial becomes a filed bug rather than a remembered flake.

**Not a #580 defect.** Measured on `main` @ `102317b4` with nothing else in the
tree: six standalone runs of `docker/qemu/run-aarch64-refusal-drain-gate.sh`,
**2 failed** (runs 1 and 6), 4 passed. The #580 branch showed the same signature
once inside a full-test run and then passed the gate 3/3 standalone.

The serial here is main's run 6. The decisive line is the oracle's own
measurement block: every ownership count is exact and the only divergence is one
extra slot **free** inside the window —

    slot_alloc_delta=1000 : slot_free_delta=1001 : slot_balance=-1

`kernel/src/tracing/providers/teardown.rs` asserts those two deltas are equal
across the stress loop, but they are read from **global** slot counters while the
rest of the system is live, so a concurrently reaped thread returning its
kernel-stack slot inside the window breaks the equality without anything being
wrong with the workload. Whether that is the whole story — or whether a slot can
genuinely be returned unpaired — is the open question recorded on #646; a repair
that only narrows the measurement window would hide the second reading.

Phase 5 of `run-aarch64-full-test.sh` is a merge gate, so this reddens roughly a
third of merge attempts. It is **not** a pre-adjudicated tolerance signature and
must not become one.
