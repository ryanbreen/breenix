# A.3 host QEMU review repairs — a3-u39

ARM ledger coordinator line 344 governs this host-only unit. The private
[QEMU fork](https://github.com/ryanbreen/breenix-qemu) retains its QEMU licenses.
Breenix contains the download recipe and host checks, without QEMU patches.
The existing `arm-catchup/a-3` branch continues the UART repair at `23620915`.

## Fork history and disposition

New commits descend from `fc43bd4cc6483ee30a6150d5524dbde62e25647b`:

- `c5c70a1c9`: check-first renderer reset and cache reproductions.
- `160f45890824de1270a2f1e84b9d02a12b2219d2`: runtime repairs and a tracked
  standalone cache-pin header; this is the recipe's source pin.
- `da559b033`: extend the lifetime check for exit shutdown and unsupported
  async callback APIs, with failing results against the old source.

| Review item | Disposition |
| --- | --- |
| M1 | Guard the completion BH with the virtio transport's reentrancy guard; assert against renderer self-quiesce. |
| M2 | Reject builds lacking virglrenderer 1.1.2 / callback API v4; initialization also rejects a worker whose async flag or callbacks are inactive. |
| m3 | Poll the renderer's I/O-handler AioContext instead of nesting the main loop; mark the GL device non-hotpluggable. |
| m4 | Refuse to pop control descriptors while quiescing, including after initialization yields. |
| m5 | Register an exit notifier that joins the worker; remove it on unrealize. |
| m6 | Use QEMU's joinable thread API and release worker EGL thread state on exit. |
| m7 | The pinned ANGLE Xcode response files lack the share-context-lock and context-mutex defines. Finish producer GL work before the display consumes a shared texture. |
| m9 | Keep a rolling 128 attestation records, limit each GL string to 1024 bytes, cap tracked submitted contexts, and clear state on renderer reset. |
| m10 | Track the cache-pin header in the fork; use nonblocking process locks, report an unavailable cache extension, and reject a truncated `confstr` result. |
| Cache eviction | Update use time on a successful value fetch; exclude the replaced key's old record from incoming-budget accounting. |

[Private fork issue 1](https://github.com/ryanbreen/breenix-qemu/issues/1)
records m8's GTK context-type limitation. The UTM/SPICE EGL path is the unit's
scope. Replacing the display-handoff Finish with a fence is a future optimization.

## Check-first evidence

The fork's `tests/breenix/test_renderer_lifetime.py` compiles production BH setup,
completion and quiesce bodies with a forced response-to-transport MMIO reset.
On the baseline the response and direct self-quiesce enter the bounded deadlock
sentinel (exit 77). The fixed guarded path returns, and direct self-quiesce asserts.
The harness models the transport guard and coroutine entry; it does not execute a
guest exploit. Its exit case invokes the registered production notifier. Its API
case preprocesses the production version gate: two unsupported combinations fail
only after the repair; the supported combination already compiled on the baseline.

The fork's `tests/breenix/test_blob_cache.py` executes the production cache header.
Its five baseline failures force a held cross-process lock, same-key replacement
at the budget, use-time refresh, unavailable-extension logging, and a truncated
cache-directory result. The repaired cases pass. The lock holder releases on pipe
EOF so the bounded failing run leaves no lock-holder process behind.

Breenix check-first commit `42ade847` requires b6 in the recipe and launch gate;
its initial run fails against b5. Existing deadlines and guest checks are unchanged.
The renderer-isolation harness now links QEMU's thread implementation and models
the coroutine branch used by production SUBMIT_3D, retaining its MMIO-progress,
ordered-completion and drain assertions.

## Build and final host checks

The clean-clone b6 release build completes in 652 seconds with exit 0 and signature
verification. It has 81 distinct warning lines, with no new lines relative to the
retained b5 build log; these are dependency/SDK diagnostics. The changed GPU sources
compile without a new warning. The installed binary SHA-256 is
`22d535323fa9ea1621c05b810d217818d58e3fed01178b68b5487ae8dcb60877`.

No-guest checks pass: EGL initialization 1/1, persistent cache 1/1, renderer
isolation 1/1, external-source contract 1/1, and launch gate 4/4. The fork's
lifetime/API checks pass 2/2 and cache check passes 1/1. Cache population writes 16
blobs; the fresh-process run reads 32 hits and writes no blobs. The three modes
link 16 programs in each of two contexts. The gate uses b6's exact binary identity.

The b6 build and host-check identities are recorded in
[the result record](serials/a3-u39-qemu-review.json).
This unit runs no guest boot. ARM window-manager and native acceptance belong to
the acceptance unit; earlier captures do not establish acceptance of this host pin.
No kernel or guest program source changes here. The scoped build is the ARM64
host QEMU release bundle.
