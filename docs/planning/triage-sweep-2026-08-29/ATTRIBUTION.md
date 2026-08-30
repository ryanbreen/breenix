# Triage sweep — x86 gate red attribution and #670 runtime coverage

Branch `chore/triage-sweep-1` @ `671bab95`, based on `main` @ `782ab96f`.
Round-1 verification blocked on two things: two unattributed x86 gate reds, and a
runtime-proof claim for #670 that had never been executed. This document records
what was measured for both.

All x86 runs are on beast, container `breenix-x86`, KVM, `-cpu host`,
`--features testing,external_test_bins`, `BREENIX_NET_MODE=none`, 150 s per boot,
verdict by `scripts/x86-gate-verdict.sh` with `EXPECTED_EXITS=10`.

---

## 1. The two reds

### Signatures, verbatim

| # | gate verdict line | serial evidence |
|---|---|---|
| A | `x86 userspace gate: FAIL - failing process is not allowlisted: clock_gettime_test` | `TEST_TALLY: exited=19 nonzero=1 failed=[clock_gettime_test:1]` |
| B | `x86 userspace gate: FAIL - failing process is not allowlisted: /usr/local/test/bin/clon` | `CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` → `failed=[/usr/local/test/bin/clon:1]` |

Signature B is `userspace/programs/src/clonevm_exec_test.rs`'s post-exec
assertion that `futex_wait_with_timeout()` on a never-woken word returns
`ETIMEDOUT`; the parent exits 1 immediately on mismatch. The truncated process
name is the second-stage exec's `argv[0]`.

### Rates, both arms

Round 1 (9 branch boots, 5 main boots) plus round 2 (14 branch boots, 14 main
boots), same host, same profile. Round 2 ran as interleaved blocks — branch 7,
main 7, main 7, branch 7 — so host drift cannot be read as an arm effect.

| signature | branch `671bab95` | main `782ab96f` |
|---|---|---|
| A `clock_gettime_test:1` | 1/23 | **2/19** |
| B `CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` | 3/23 | 0/19 |
| (C) `loopback_wake_test_child:15, loopback_wake_test:1` | 1/23 | 1/19 |

Failing serials for every round-2 red are preserved under `serials/` here.
Round 1's two signature-B serials were overwritten before they were copied and
no longer exist; round 2's `BRANCH-blk2-boot7` replaces them.

### Verdict A — pre-existing, #631

`clock_gettime_test:1` occurs on pristine `main` at 2/19 against the branch's
1/23. It is the exact signature of the already-open **#631**
("x86: clock_gettime_test exits 1 on a boot that otherwise passes ... the test
does not say which assertion failed"), and #631 is not the first sighting
either: `docs/planning/green-program/sockets/EVIDENCE-2026-08-29.md` §7 records
it at 1/24 on the #568 candidate and 1/16 on the reverted arm and attributes it
to #631 "already filed, both trees". Attributed; no new issue.

### Verdict B — pre-existing, and branch-causation is excluded by construction

Signature B was not observed on main in this battery (0/19), so rate alone does
not settle it: 3/23 against 0/19 is p ≈ 0.24 by Fisher's exact test, which
discriminates nothing. It was settled by measuring the branch's kernel delta
instead.

Against this gate profile the branch carries exactly one compiled kernel change
that the boot could reach: #670's `validate_fd_for_degenerate_transfer()`, called
from the `buf_ptr == 0 || count == 0` guard of `sys_read`, `sys_write`,
`sys_pread64` and `sys_pwrite64`. (#665's change is inside
`#[cfg(feature = "interactive")]`, which this profile does not build; #679's is
in `kernel/build.rs` and runs at build time; #678's touches no compiled code.)

That call was instrumented directly. A local, uncommitted probe on the branch
bytes added two relaxed atomic counters — one incremented on entry to the
validator, one on its `EBADF` arm — and printed them beside `TEST_TALLY`:

```
PROBE boot1 :: DEGENERATE_TRANSFER_PROBE: entries=0 ebadf=0 :: TEST_TALLY: exited=19 nonzero=0 failed=[]
PROBE boot2 :: DEGENERATE_TRANSFER_PROBE: entries=0 ebadf=0 :: TEST_TALLY: exited=19 nonzero=0 failed=[]
PROBE boot3 :: DEGENERATE_TRANSFER_PROBE: entries=0 ebadf=0 :: TEST_TALLY: exited=19 nonzero=0 failed=[]
```

Nothing in the profile issues a zero-length or null-buffer transfer, so **no
instruction the branch adds executes in this gate at all**. The two arms differ
only in code layout. A red that appears on one of them and not the other is
therefore sampling noise by construction, whatever the counts happen to be.

This matters specifically because #670's own D2 adjudication predicted the
opposite — that moving the descriptor lookup ahead of the zero-count return would
reshape exactly this clonevm second-stage stall ("RDX tracks the watched word, so
a non-zero word yields the `EBADF → ENOSYS` fixed point instead"). The prediction
does not apply here: `clonevm_exec_test` never calls `read`/`write` with a
degenerate length. `spin_until_u32` spins on `sys_yield`, and the program's only
`write` calls are `raw_msg`, which always passes `msg.len()`.

Signature B also has main-lineage prior sightings under a different name: the
string `CLONEVM_EXEC_TEST: ERROR futex timeout did not return ETIMEDOUT` is the
central specimen of the #608 RCA (four independent captures across its A/B/B3
arms), and #690 is the same second-stage rendezvous failing on aarch64. #608 was
closed as not-reproduced and #690 is aarch64-only, so the live x86 face has no
open home and is filed fresh.

---

## 2. #670's runtime coverage

`b96c9c58` claimed `dup_test` Phase 6b as the runtime coverage #670 asks for
("a zero-length read and write on a closed fd expecting EBADF, and on an open fd
expecting 0"). Phase 6b exists and is correct. **It does not execute on either
architecture's gate**, and the claim is withdrawn.

### x86 — the call site is unreachable, not slow

`test_exec::test_dup()` is called from `kernel/src/main.rs:1550`, inside the
`#[cfg(all(feature = "testing", not(feature = "interactive")))]` block. Control
never arrives there. Measured on a completed, passing gate boot:

* `=== IPC TEST: dup() syscall functionality ===` — 0 occurrences.
* `Dup test: process scheduled for execution.` — 0 occurrences.
* last self-test marker emitted: `=== MULTIPLE CONCURRENT PROCESSES TEST ===`.
* `TEST_TALLY: exited=19` — the nineteen processes are the disk-loaded ones.

The mechanism is documented in the tree already, at `kernel/src/main.rs:1063`:
"the boot thread then strands inside the disk-backed `get_test_binary()`
busy-wait in `test_exec::test_direct_execution()` and never returns." That is
**#508**, open, which names this precise consequence ("so `test_exec` self-tests
may not complete"). It is not the 150 s boot cap: the cap kills boots that pass
the gate too, and those boots also show zero dup markers.

### aarch64 — no gate builds the profile that would run it

`dup_test` is in `kernel/src/boot/test_list.rs`'s shared `TEST_BINARIES`, is
installed to `/usr/local/test/bin/dup_test` by `scripts/create_ext2_disk.sh`, and
`main_aarch64.rs`'s `load_test_binaries_from_ext2()` would create a user process
for it. That function is `#[cfg(feature = "testing")]`, and **every automated
aarch64 gate builds `--features boot_tests`** — full-test, strict, service
sequence, per-CPU stack custody, refusal drain and the prod profile alike. So the
roster never loads.

Two further measurements, both at the branch bytes:

* The one script that does build `--features testing`,
  `docker/qemu/run-aarch64-test-suite.sh`, was silently broken. It selects a test
  by rewriting a `run_userspace_from_ext2("/bin/init_shell")` call in
  `main_aarch64.rs`, and that function does not exist anywhere under `kernel/`
  any more. The substitution matched nothing, so every "test" it ran was the same
  unmodified boot and the PASS/FAIL it printed described that boot rather than the
  named test. It now asserts the rewrite and aborts when it does not apply; the
  re-plumbing is filed.
* A raw `--features testing` aarch64 boot does not reach userspace loading at
  all. It panics first in the in-kernel tests — `kthread_tests.rs:75 "kthread
  never started"` on one CPU, `softirq_tests.rs:228 "ksoftirqd should have
  processed deferred softirqs"` on four.

### Honest coverage statement for #670

The fix is proven by `tests/degenerate_transfer_fd_validation_structure.rs`: a
shape rule over `handlers.rs` with no allowlist, with all four sites mutated
singly back to the pre-fix shape and each singly reddening the oracle and naming
its own handler, plus a census companion and a negative test. **There is no
runtime path on either architecture today**, and the probe above proves the arm
is never entered in the x86 profile even when the whole gate runs.

`#670` is therefore **not closed** by this branch. Its own ask includes userspace
coverage, and per this project's standing rule test wiring must be proven to
execute in the gate's actual feature profile. The commit lands the fix and the
coverage; the issue stays open on the wiring, blocked behind #508 on x86.
