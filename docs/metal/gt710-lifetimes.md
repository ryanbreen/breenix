# GT 710 memory lifetimes: milestone 5 PR 5.4

`lifetime::Context` owns a private VM and references its instance, directory,
small-page tables and mapped backing. Allocator pins retain the existing
physical charge; a reference or BAR view is not a second allocation. Releasing
a pinned allocation is refused. A successful remap releases the old mapping
reference only after publication and invalidation; failure retains both old
and replacement backing. Unmap follows the same ordering.

Queued work, read-back and scanout consumers receive tickets sharing the
context's residency. The context retains a separate reference for each pending
consumer, so dropping a caller ticket cannot cancel work. Mutation and
retirement refuse outstanding consumers. Completion tickets belong to one
residency and a monotonically increasing sequence; an address reused by a new
context cannot accept the previous owner's ticket.

An EIO, attributed fault or failed reset stops admission and retains charges.
Attribution matches the full decoded live instance address; an unknown instance
is not guessed to belong to another context. Fault-register reads remain
read-only. Successful detach invalidates while table and backing references
are still held, then releases them and reclaims the tables. Failed detach
retains the owner for later recovery. Dropping an unretired owner deliberately
leaks its residency reference, retaining storage instead of permitting reuse.

The device admission, completion, drain and confirmed-reset entry points are
unsafe kernel-internal APIs with explicit obligations. Milestone 6's scheduler
must retain this owner before publishing a channel/binding, supply an
engine-ordered completion for the exact ticket, and confirm disabled
scheduling/bindings plus stopped device access before drain/reset retirement.
The API does not implement or certify those hardware mechanisms in milestone 5.
CPU completion and cancellation refuse a context ever admitted to a device.
A CPU-only cancellation cannot clear a poisoned VM publication.

The existing suite-only VM probe uses the context owner for its two private
spaces. Before remapping, it holds one consumer of each kind, refuses backing
and table release and VM mutations, then exercises synthetic EIO, synthetic
instance attribution and failed-reset notification. Charges remain at peak
through consumer release; the unaffected context accepts a CPU read-back hold.
Complete instance/directory/SPT bytes are compared again through PRAMIN and
BAR1 against the original snapshots. Only these never-submitted CPU spaces may
cancel the synthetic fault and finish the existing remap/restore/retire checks.
The original VM capture still requires boundary patterns, nonaliasing windows,
unchanged excluded storage/guards, empty hardware fault masks and exact baseline
reclamation. The aperture and native display/console gates remain required.

`GT710_LIFETIME` records the hold/charge sequence, blocked release counts,
synthetic fault attribution and retained table snapshots. The existing metal
scorer adds `lifetime/retention` and `metal-gt710/lifetime`; it requires the
corresponding complete VM capture. `exercise` is `cpu-only`, and both
`translation` and `reset_execution` are `pending-m6`. There is no executable
channel, deliberate hardware page fault, permission probe or measured reset in
this step. Same-VA/different-backing channels, remap/invalidation under execution,
unmapped-page fault isolation and a clean subsequent CE copy remain in #2514.

The lifetime/allocator policy is Breenix-owned. Existing hardware encodings,
fault decode and BAR operations retain their original checked MIT notices and
sources in `THIRD_PARTY_NOTICES` and `docs/gui/provenance/gt710-vm.md`.
No new third-party implementation is introduced. This document specifies the
mechanism; final-head measurements belong to the PR and private boot record.
