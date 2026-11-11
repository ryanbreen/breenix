# A.3 u2: serial host channel declaration

The suite's host-key transport name is the part before `;`, as in
`libs/libbreenix/src/suite.rs`. The kernel now arms serial host input only
for `host-channel`, while passing the complete declaration to the suite.
The R240 suite boot remains unchanged: `scripts/metal/metal_root.py:46`
sets `HOSTKEYS = 'host-channel'`.

The focused host check failed on `qmp` with a temporary predicate implementing
the old `hostkeys.is_some()` rule, then passed with the production predicate.
The first commit contains the check; the temporary predicate was not committed.

The A.3 u1 baseline passed all input and pointer cases. No recorded u1 failure
is shown to be cleared by this change: performance/latency-under-animation,
recovery/restart-adopts, recovery/supervisor-restarts, and
liveness/memory-bounded remain unproven here. Acceptance is deferred to the
acceptance unit; no boot was run in this unit.

The scoped x86 and ARM64 release kernel builds succeeded. Cargo reported the
pinned Rust toolchain's existing `core` future-incompatibility warning for
NEON target-feature annotations on ARM; `cargo report future-incompatibilities
--id 1` attributes it to the toolchain's generated `core_arch` source, not
this change. Neither build emitted a warning from the kernel.
