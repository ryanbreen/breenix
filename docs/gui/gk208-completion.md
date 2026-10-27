# Native CE completion and observation

M6 PR 6.3 starts the native adapter at the shared request/completion and
observation boundary. `gpu_completion` and `gpu_observation` compile on both
architectures. The ARM device adapter uses the shared caller/native echo matcher
and the common byte-difference routine; the native CE adapter uses typed engine
release evidence. The complete syscall device dispatch and native public ABI row
remain M8.1 work.

A CE request carries its caller fence separately from the channel's 32-bit queue
sequence, instance identity, reset epoch, resource identity and generation.
The waiter reads the device's release words, channel binding, consuming PBDMA
identity, GET and hardware error state before constructing a completion. Neither
GET alone nor an old epoch certifies a transfer. A native completion has an
`a0b5-engine-release` evidence kind, without a virtual response status.

The bring-up adapter serializes writes and observations. A completed destination
write advances its resource generation. A read-back hold pins lifetime across both transfers
and record construction; the serialized caller supplies write exclusion. Each transfer poisons the pinned system destination
with 0x55 or 0xaa, executes CE, checks its genuine release, and reads through the
shared invalidate-before-copy boundary. Record construction requires ordered,
distinct requests for the writer and both transfers, matching context, epoch,
resource and generation, equal complete lengths and byte equality.
The existing independent host oracle checks complete destination SHA-256 values,
including untouched bytes and pitch gaps. The retained record links those
transfers to their actual native sequence values and separate caller fences.

The 6.3 gate is `metal-gt710/completion`, backed by the `gt710` suite's
`completion/observation` case. It also requires the existing CE transfer checks.
The native controls mutate software request/publication fields around a real
completed transfer. They check matcher rejection and publication validation;
hardware isolation evidence comes from the separate private-channel and fault
probes, not those software controls.
Existing withheld-release, stale-epoch and real unmapped-page controls run
through the same native request matcher.

Issue #2514's engine checks continue to execute: same VA in sequentially scheduled
private channels, remap/invalidation, system-memory DMA, actual unmapped-page
fault attribution, confirmed engine reset and restored/surviving copies. Native
CE execution does not establish PGRAPH execution. IRQs stay masked in this
boot-owner probe; no decode or recovery is added to a top half.

Concurrent contexts, ring wrap/backpressure, killable waits and deferred client
destruction belong to 6.4. The complete 2 s watchdog, further 2 s no-progress
policy and general confirmed-reset gate belong to 6.5. This adapter retains the
existing bounded CE fault/reset probe and its 100 ms reset limit; a service power
cycle is experiment recovery only.
