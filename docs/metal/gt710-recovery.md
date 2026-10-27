# Native CE fault and reset gate

M6 PR 6.5 establishes recovery for the currently admitted CE-only transport.
It introduces no CE firmware and does not claim PGRAPH recovery or execution.
The display engine retains its native owned scanout; a host power cycle is
experiment cleanup and is not a reset receipt.

The root suite probe opens `/proc/gt710-recovery` after the existing killed-wait
check has drained. A native owner worker takes the allocator and channels;
the reader sleeps on a killable wait queue. FIFO/CE interrupts stay masked for
this bounded bring-up path. Fault decoding, scheduling removal and recovery
run on the worker, with no interrupt-handler changes. The interface accepts
no GPU addresses, MMIO values or command streams.

A one-byte read from an unmapped GPU page produces an attributed CE MMU fault
while two channels are scheduled. Quarantine refuses new publication, stops
the bad channel and retains its outstanding hold. Local recovery resets only
the enumerated CE, publishes a survivor-only runlist, confirms removal and
checks complete P/~P system-memory readbacks on the original surviving context
and epoch. Firmware-owned storage is not written.

A separate yielding acquire withholds real channel work. The watchdog scans
at scheduler wakes spaced by 10 ms, quarantines after 2 s and removes that
channel from scheduling without treating removal as device completion. The
survivor completes complementary full transfers and advances the no-progress
clock. Its next request queues eight private 16 MiB copies behind an acquire. After a
further 2 s without an ordered completion, the worker releases that acquire and
requires a changed destination nonce, a busy engine and an absent final hardware
release. Transport reset then fails both outstanding fences
with `device-reset`. The receipt validator first rejects real pre-reset observations, with backing
still pinned; the same validator judges the actual reset readbacks. Both raw
release records and budget-0 hardware completion observations precede EIO.
Reset-enable readbacks, cleared bindings, idle runlist/PBDMA/engine and absent
fault/error state precede the trusted reset receipt.

Replacement command storage and mappings are prepared before that decision,
then published only after confirmed cessation of old engine access. The 100 ms
budget includes reset from observed active copy work, control-channel republication
and EIO outcomes. Scheduling is restored on removal errors before CE-reset
escalation; backing remains retained until the final reset receipt. Fresh
channels have distinct reset epochs. A stale epoch planted in an otherwise
valid release is rejected before clean complementary copies on both new
channels. The worker drains and reclaims its exact allocator baseline.

`GT710_RECOVERY` carries raw reset and fault observations, timing intervals,
holds, context/epoch identity and complete-buffer SHA-256 digests. The retained
host scorer rejects incomplete records. The watchdog quarantine interval remains
2,000..2,500 ms, no-progress 2,000..2,250 ms and stalled-submission recovery
4,000..4,850 ms. Reset is at most 100 ms. Console acceptance remains at most
330 s from handoff, under the unchanged 420 s boot deadline.

These mechanisms extend the device-executed memory checks tracked by #2514:
real unmapped-page attribution with a concurrently scheduled survivor, retained
backing through quarantine and local reset, and fresh-epoch transport recovery.
Native rendering and the later shared-core adapter remain separate milestones.
