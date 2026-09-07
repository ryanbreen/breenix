# #908 — mask UDP registry holds and refuse contended NetRx lookups

Branch `net/908-udp-ports-lock`, based on `origin/main` at `19d13f0ee64a`.
Implementation, oracle, ratchet, gate pins, fixture and aarch64 evidence are
commit `409fb7724a6f93f53cb6c09e4336f2d118104ba5`. This document and the x86
captures form the second commit. The worktree stayed on this branch.

## The defect and holder census

`SOCKET_REGISTRY.udp_ports` is a private
`Mutex<BTreeMap<u16, (ProcessId, SocketHandle)>>`, separate from #823's outer
`Mutex<UdpSocket>`. Before this repair, `bind_udp`, `unbind_udp` and
`lookup_udp` each acquired the registry with a blocking `.lock()`.

At the implementation commit, the physical acquisition census is **2 of 2**:
`kernel/src/socket/mod.rs:126`, the masked primitive's blocking acquisition,
and `kernel/src/socket/mod.rs:165`, the IRQ-route try-lock. The following
rows account for their production and boot-test uses:

| API or reaching path, at this commit | context and previous protection | repaired protection |
|---|---|---|
| `socket/mod.rs:131` `bind_udp`, called by `socket/udp.rs:115` | Thread context. Already indirectly masked in `sys_bind` by the outer `with_locked_masked` at `syscall/socket.rs:298`, verified in source before editing | Calls `with_udp_ports_masked` at the registry boundary; includes the nested `next_ephemeral` lock, bounded ephemeral scan and allocating map insertion |
| `socket/mod.rs:152` `unbind_udp`, via `socket/udp.rs:231` `UdpSocket::Drop`, reached through `Process::terminate` → `close_all_fds` (`process/process.rs:464,474`) | Thread teardown under `with_process_manager`; masked before this change | Remains masked, with registry masking independently enforced |
| Same `Drop` → `unbind_udp`, reached through `task/process_task.rs:698` `close_extracted_fds`, called at line 901 by `handle_thread_exit` | Explicitly outside the PM lock; the UDP fd falls through the wildcard arm, and dropping its last Arc could acquire this registry unmasked | Registry primitive masks the hold; the cleanup caller needs no change |
| `net/udp.rs:121` `handle_udp` → `socket/mod.rs:164` `try_lookup_udp` | NetRx softirq route, before `deliver_to_socket` enters `with_process_manager`; previously a blocking `lookup_udp` | Uses `udp_ports.try_lock()`, counts refusal and drops a contended datagram |
| Existing boot-test cleanup callers in `test_framework/registry.rs:5146,5162,5955,5971`, and new callers at `5513,5529` | Test-thread `unbind_udp` calls, outside a registry hold | Same API signature; each acquires through the masked primitive |
| New holder at `test_framework/registry.rs:5553` | Boot-test peer kthread, new in this round | Calls the exact production primitive at `socket/mod.rs:116` |

Source paths in this table are relative to `kernel/src/`. The acquisition
count above concerns `udp_ports`, not the separate `next_ephemeral` mutex.
`alloc_ephemeral_port` receives the already-locked map; it does not reacquire
`udp_ports`. The caller search found 1 production lookup caller and 2 direct
production bind/unbind callers, with no re-entrant registry acquisition in
those bodies. The mutable-map argument coerces to the helper's immutable
map reference as written; the aarch64 and x86 builds compiled it.

**Correction to the live issue body:** `gh issue view 908 --comments` was
run, and the body was also read through `--json body,title,state`. Its
claim that deferred reclamation reaches `close_extracted_fds` is stale.
The source and #823's review correction agree that this is ordinary
`handle_thread_exit` cleanup. This round follows that source, rather than
repeating the stale deferred-reclaim chain. The actual lookup line also
moved from the issue's old line number to line 121.

## The repair and departure from #823

#823 exposed the outer mutex through the fd's socket Arc and needed an
opt-in masked primitive at 4 caller sites. This registry's private map is
encapsulated by 3 methods. Masking belongs at its own boundary: 2 of 2
thread-side holding methods, `bind_udp` and `unbind_udp`, call the new
`pub(crate) with_udp_ports_masked` primitive:

```rust
Cpu::without_interrupts(|| f(&mut self.udp_ports.lock()))
```

Its crate visibility lets the oracle exercise production code directly.
The architecture aliases and `CpuOps` usage mirror #823's primitive.
The closure includes acquisition, the map operation and guard destruction
before interrupt state is restored.

The IRQ decision follows [CLAUDE.md's interrupt rules](../../../../CLAUDE.md),
particularly “NO locks that might contend (use `try_lock()` with direct
hardware fallback)” and its review check for a try-lock fallback. For this
NetRx UDP route, #908 specifies dropping the best-effort datagram on
contention. This constraint decides the lookup design independently of any
argument about present-day same-CPU scheduling behavior.

`try_lookup_udp` returns 3 distinct outcomes: contention, an unbound port,
or an owner. A contended acquisition increments the relaxed
`UDP_PORTS_LOOKUP_REFUSED` counter; `udp_ports_lookup_refused()` reads it
for diagnostics. The counter is not a synchronization mechanism.
`handle_udp` handles the 3 outcomes explicitly.

`deliver_to_socket` remains byte-identical to the base version, verified by
comparing its source suffix: it takes the separate outer `socket_ref.lock()`
at `net/udp.rs:170`, inside `with_process_manager` at line 155.

## The oracle

`udp_ports_lock_oracle` is boot_tests-only and registered immediately after
`udp_socket_lock_oracle` in the same array, at `TestStage::ProcessContext`
(`test_framework/registry.rs:10234`). It needs a live process row for packet
delivery, and the subsystem's sequential tests keep its socket borrow from
overlapping the adjacent PM/console oracles.

The test binds port 54560, with synthetic source port 54561 and payload
`breenix-908`. The holder is pinned to a live peer, preferring a nonzero
peer when possible, and preempt-disabled. It calls the production registry
primitive, reads `!arch_interrupts_enabled()` inside the hold, publishes
`ACTIVE`, and spins for 12,000 microseconds using the shared CNTVCT helper.
It records elapsed time, clears `ACTIVE`, releases the lock and publishes
`DONE`.

The orchestrating thread sends loopback datagrams at approximately 1 ms
intervals during the hold and for a 20 ms settle margin afterward. Its
interrupts remain enabled so its NetRx softirq can run. Its CPU is sampled
once while `ACTIVE` is true; a short preemption bracket prevents migration
across that flag/CPU snapshot without masking interrupts. It then releases
test affinity, joins the holder with a bounded wait, and reads the refusal
counter delta. Receive-queue inspection uses #823's outer masked primitive
and filters packets by payload before counting. Each attempt clears prior
queue evidence.

PASS requires armed and initially interrupt-enabled state, `masked_in_hold=1`,
a send, at least 8,000 us of hold, `refused>=1`, a delivered matching packet,
`stalled=0`, `hold_done=1`, `joined=1`, and a measured driver CPU distinct from
the holder. Up to 3 attempts are available for missing contention; a measured
missing mask is reported without retrying it away.

### x86 arm

The x86 gates boot `-smp 1`: there is no second CPU for this contention
experiment. This differs from #823/#812's preempt-count-based SKIP reason.
`X908_UDP_PORTS_LOCK_ORACLE_RAN` and `run_udp_ports_lock_oracle_x86_once`
in `executor.rs` copy the existing latch-once pattern; the call follows
`run_udp_lock_oracle_x86_once` in `advance_stage_marker_only`.

```text
[UDP_PORTS_LOCK_ORACLE:x86:arm=none:reason=uniprocessor_no_udp_ports_contention_peer:online_cpus=1:SKIP]
```

That literal occurs exactly once in
[06b-x86-boot-tests-serial-user.txt](serials/908/06b-x86-boot-tests-serial-user.txt).
SKIP is not an x86 contention-oracle PASS.

## Red and green — the measured mutation result

The behavioral mutation removes the production primitive's interrupt mask,
leaving `f(&mut self.udp_ports.lock())`. Removing that expression also leaves
the architecture aliases and `CpuOps` import unused: the first diagnostic
build printed 2 warnings. Those unused declarations were removed in the
temporary red source before the scored boot, with no warning suppression.
The warning-clean red build is preserved in
[08b-a64-red-build.txt](serials/908/08b-a64-red-build.txt). It has only the
expected upstream `core` future-incompatibility notice.

The final red run used `BREENIX_GATE_SKIP_STRUCTURE=1`, because the deliberate
mutation fails the ratchet. It exited 1:

```text
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:driver_cpu=3:irqs_enabled_before=1:masked_in_hold=0:sends=21:hold_us=12005:refused=8:delivered=12:stalled=0:hold_done=1:joined=1:FAIL]
```

Source capture:
[01-a64-red-unrepaired-serial.txt](serials/908/01-a64-red-unrepaired-serial.txt).
Gate output: [01b-a64-red-gate.txt](serials/908/01b-a64-red-gate.txt).

**No holder stall was observed.** The measured result is
`stalled=0:hold_done=1:joined=1`; contention was exercised (`refused=8`).
The lookup's try-lock returns on contention instead of waiting on the
holder, so removing this mask does not reproduce #823's blocking-consumer
wedge. `masked_in_hold=0` is the actual defect discriminator. The kernel
oracle reports FAIL from that condition, and the gate also pins the literal
`masked_in_hold=1`. The gate stopped on the boot-test failure; this is not a
claim that the entire mutated boot completed its later stages.

The source was restored byte-for-byte from the saved green file (`cmp`,
exit 0), including the aliases/import. The 14-test ratchet passed again and
the kernel was rebuilt. Strict ×3, with structure preflight enabled, exited
0 and reported `PASS: 3/3 boots succeeded`:

```text
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=2:driver_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=22:hold_us=12006:refused=9:delivered=13:stalled=0:hold_done=1:joined=1:PASS]
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=2:driver_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=21:hold_us=12022:refused=7:delivered=14:stalled=0:hold_done=1:joined=1:PASS]
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=2:driver_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=21:hold_us=12010:refused=10:delivered=11:stalled=0:hold_done=1:joined=1:PASS]
```

In order, those are
[02-a64-green-strict-boot1-serial.txt](serials/908/02-a64-green-strict-boot1-serial.txt),
[03-a64-green-strict-boot2-serial.txt](serials/908/03-a64-green-strict-boot2-serial.txt),
and [04-a64-green-strict-boot3-serial.txt](serials/908/04-a64-green-strict-boot3-serial.txt).
Combined gate output:
[04b-a64-green-strict-gate.txt](serials/908/04b-a64-green-strict-gate.txt).

### Development readings retained

The first repaired boot failed because the initial driver implementation
overwrote `driver_cpu` on sends after the hold had ended. The thread migrated
onto the former holder CPU during the settle margin:

```text
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:driver_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=21:hold_us=12006:refused=7:delivered=14:stalled=0:hold_done=1:joined=1:FAIL]
```

This was an oracle snapshot defect, not a discarded host flake. It is
preserved in
[00-first-green-driver-snapshot-fail-serial.txt](serials/908/00-first-green-driver-snapshot-fail-serial.txt)
and [00b-first-green-driver-snapshot-fail-gate.txt](serials/908/00b-first-green-driver-snapshot-fail-gate.txt).
The correction samples during `ACTIVE` as described above. The final red
and strict ×3 runs both used that corrected implementation.
An earlier red run, also with a completed holder and `masked_in_hold=0`, is
retained in [01c-initial-red-serial.txt](serials/908/01c-initial-red-serial.txt)
and [01d-initial-red-gate.txt](serials/908/01d-initial-red-gate.txt).

## Gate pins

The 4 of 4 requested scripts carry the new marker beside #823's marker:

| script in `docker/qemu/` | enforcement |
|---|---|
| `run-aarch64-boot-test-strict.sh` | Binary marker census, explicit FAIL scan and PASS regex. Requires `masked_in_hold=1`, positive sends/refusals/delivery, completed/joined holder and no stall. Hold regex enforces 8,000 us using a four-digit 8/9-leading value or a five-or-more-digit positive value. Self-checks reject `hold_us=0` and `refused=0`, and accept a positive-duration sample |
| `run-aarch64-prod-profile-boot-test.sh` | New literal counted and required at 0; early and final count output added |
| `run-x86-boot-tests.sh` | Exact x86 SKIP literal in the pass chain and matched-line output |
| `run-x86-prod-profile-boot-test.sh` | New prefix in the forbidden test-only marker census |

The strict regex's CPU fields do not independently compare CPU identities;
the kernel PASS predicate performs that comparison. Synthetic self-check
samples test the scorer, while the linked QEMU captures supply runtime
measurements. Four shell syntax checks passed.

## The ratchet

`tests/udp_ports_lock_irq_structure.rs` has 14 tests:

1. Whole-hold mask in the production primitive, plus mask-removal mutation.
2. `bind_udp` uses that primitive without a raw map lock, plus mutation.
3. `unbind_udp` uses that primitive without a raw map lock, plus mutation.
4. Lookup uses `try_lock` instead of `lock`, plus blocking-lookup mutation.
5. The refusal arm increments the relaxed counter, plus increment removal.
6. `handle_udp` calls the new lookup name, plus restoration of the old name.
7. The copied regression guard for `deliver_to_socket`'s outer lock.
8. A green control invoking the 7 rules.

Rules 1–6 each assert that their `replacen` mutation changed real source,
then reject it with the same predicate used for the green rule. `code_mask`
was copied byte-for-byte from the #823 file and its identity was checked.
The new registry-method rules extract balanced bodies after masking comments
and literals. The copied rule 7 retains #823's stated limitation: its PM
check is a whole-file substring search, not a nesting-aware proof, and it
has no separate mutation test.

## Fixture re-record (R182)

The fixture search found 4 affected suites: `tty_irq_pm_structure`,
`tty_irq_fg_structure`, `loopback_pump_structure`, and
`ttbr0_shadow_reconciliation_structure`. They each read
`tests/fixtures/udp-socket-lock-aarch64-serial.txt`.

The new `score_serial` requirement rejected the old serial for its missing
UDP-ports marker. More precisely, fixture score-only mode bypasses the
binary `require_boot_tests_kernel` census; it was the new serial PASS pin
that rejected these fixtures. Both requirements were checked rather than
assuming a new binary marker would be harmless to replay suites.

**Exactly 1 fixture was re-recorded:**
`tests/fixtures/udp-socket-lock-aarch64-serial.txt`, in place, from the real
repaired boot preserved in
[00c-fixture-rerecord-serial.txt](serials/908/00c-fixture-rerecord-serial.txt).
Its gate passed with the preflight skip explicitly set to break the stale
fixture dependency; output is
[00d-fixture-rerecord-gate.txt](serials/908/00d-fixture-rerecord-gate.txt).
The existing fixture `-text` attribute was retained; `.gitattributes` gained
`docs/planning/green-program/irq-locks/serials/908/**/*.txt -text`.
`git check-attr` confirmed `text: unset` for fixture and proof serial.

An initial sweep was not counted as green: its 4 stale-fixture suites
failed, and the first diagnostic mutation began before that sweep ended,
so the new ratchet also failed. After re-recording and restoration, a
stable full sweep passed 56/56 suites (835 tests). The final red run followed
that clean sweep. Subsequent strict and production gates used their normal
preflights. The default `run-structure-tests.sh` command tests only the
92-test teardown file; the full sweep used `gate_structure_preflight`, which
invokes the runner for each discovered suite. The base had 55 suites; this
round adds the 56th, rather than the brief's estimated 53–54.

## Gates run and landing environment

Local builds/gates used this worktree's `.tmp` and `.gate-tmp` for `TMPDIR`
and `BREENIX_GATE_TMP`. The aarch64 ELFs, font directory and ext2 image were
copied from `/Users/wrb/fun/code/breenix`; the Rust library override was
`/Users/wrb/fun/code/breenix-parallels/rust-fork/library`.

On beast, `/root/breenix` lives inside the `breenix-x86` Incus VM, reached
with `ssh beast` and `sudo -n incus exec breenix-x86 -- ...`, not directly in
the SSH user's filesystem. A fresh `/root/breenix-908` clone was made from
that source, its origin pointed at GitHub, and the pushed implementation
commit was fetched and checked out. `rust-fork` points to
`/root/breenix/rust-fork-real`; userspace ELFs and fonts were copied from
`/root/breenix`. Both temporary-directory variables were
`/root/breenix-908-tmp`. The fresh x86 build took 3m37s. The long retirement
and exec cohorts progressed to PASS without debug modifications.

| check | actual result | evidence |
|---|---|---|
| Initial claim-lint | exit 0, clean starting tree | session baseline invocation |
| Initial default structure command | exit 0, 92/92 | session baseline; restored repeat in `08c` below |
| Aarch64 `boot_tests` release build | exit 0; 0 kernel warnings/errors, expected upstream `core` future-incompatibility notice | [08-a64-build.txt](serials/908/08-a64-build.txt) |
| New ratchet, including post-restoration repeat | exit 0, 14/14 | [08c-structure-gates.txt](serials/908/08c-structure-gates.txt) |
| Restored default teardown + full sweep | exit 0; 92/92 and 56/56 suites | [08c-structure-gates.txt](serials/908/08c-structure-gates.txt) |
| Final red strict ×1 | exit 1, oracle FAIL on missing mask | [01b-a64-red-gate.txt](serials/908/01b-a64-red-gate.txt) |
| Green strict ×3 | exit 0, 3/3 PASS; preflight 56/56 | [04b-a64-green-strict-gate.txt](serials/908/04b-a64-green-strict-gate.txt) |
| Aarch64 production profile | exit 0, PASS; new marker count 0 | [05-a64-prod-profile-gate.txt](serials/908/05-a64-prod-profile-gate.txt) |
| X86 build with `boot_tests,testing,external_test_bins` | exit 0, 0 warning/error lines | [06a-x86-build.txt](serials/908/06a-x86-build.txt) |
| X86 boot gate ×1 | exit 0, frame-custody PASS; userspace `exited=110 expected>=105 nonzero=0 allowlist=0`; SKIP count 1 | [06-x86-boot-tests-gate.txt](serials/908/06-x86-boot-tests-gate.txt), [06b serial](serials/908/06b-x86-boot-tests-serial-user.txt) |
| X86 production profile | exit 0, first-attempt PASS; new marker count 0; prompt count 1 → 2 over 60s | [07-x86-prod-profile-gate.txt](serials/908/07-x86-prod-profile-gate.txt) |

The aarch64 build command was `cargo build --release --features boot_tests
--target aarch64-breenix-kernel.json -Z build-std=core,alloc
-Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`.
The x86 command was `cargo build --release --features
boot_tests,testing,external_test_bins --bin qemu-uefi`. Structure suites used
the standalone runner, not `cargo test`.

## Claim discipline

| invocation | exit code |
|---|---|
| `python3 scripts/claim-lint.py`, initial baseline | 0 |
| Tree lint before implementation commit, 26 files checked | 0 |
| `python3 scripts/claim-lint.py --commit-msg .tmp/908-fix-message.txt` | 0 |
| Final evidence tree lint before writing this document, 30 files checked | 0 |
| Tree lint including the completed document, 31 files checked | 0 |
| `python3 scripts/claim-lint.py --commit-msg .tmp/908-doc-message.txt` | 0 |

Intermediate lint attempts returned 1 for unqualified wording, generated
untracked gate-fact text, and the API's Option-return wording. The comments were
made specific, generated working artifacts were moved under `.tmp` after
preserving raw captures, and the Option contract received a #908 citation.
The clean checks above were run after those corrections. Commit messages
are used verbatim with `git commit -F` and carry both requested co-authors.

## Not claimed

* No new third-lock defect was established during this trace. The adjacent
  #812 `irq_hold_received` checker still takes the separate outer socket
  lock unmasked; that pre-existing finding is already tracked as #909 and
  was left unchanged. This round does not audit `close_extracted_fds`'s
  other `FdKind` arms or TCP socket locking.
* The aarch64 mutation demonstrates a missing mask and real contention,
  not a same-CPU deadlock. The x86 oracle remains SKIP, not cross-CPU proof.
* Contended UDP datagrams may be dropped. This is not a lossless-delivery
  claim, a throughput benchmark, or a worst-case bound on ephemeral-bind
  latency. The artificial 12 ms hold and its CNTVCT readings characterize
  these runs, not a portable timing bound.
* No merge to main or wider IRQ-lock audit is part of this round.

## Fix pass — review findings closed (2026-09-07)

This #908 review fix pass addresses two findings:

1. N-1 (MINOR/code) — close by REMOVING the one new logging call.
   Removed `log::debug!` only from `handle_udp`'s contended match
   arm. Its comment identifies the NetRx softirq/IRQ-exit context, x86
   SERIAL2 spinlock and UART work, and the existing lock-free
   `socket::udp_ports_lookup_refused()` diagnostic.
2. N-6 (MINOR/code) — close by DISCLOSING removal's allocator work in the docstring.
   Added the `unbind_udp` / `BTreeMap::remove` allocator disclosure to
   `with_udp_ports_masked`'s doc comment, including the heap allocator's
   nested interrupt mask. The function body and prior doc lines are unchanged.

`handle_udp_contended_arm_has_no_logging` pins N-1 after the existing
`body()` helper masks comments and strings, checking the last contended match
arm for the six specified logging macros.
`with_udp_ports_masked_documents_removal_allocator_work` pins N-6 by
extracting the contiguous `///` lines immediately above the primitive and
requiring `remove`, `allocate`, and `heap.rs`. Each has a
`_rule_is_not_vacuous` twin using a scoped literal replacement on real
source; the green control invokes the four new test functions. When the
N-6 disclosure is already absent during an on-disk mutation, its twin
checks that the production rule is already false; on the fixed source it
requires the deletion to match and redden that rule.

`bash scripts/run-structure-tests.sh udp_ports_lock_irq_structure` exited
0 with 18/18 tests. Capture:
[09-fixpass-structure-suite.txt](serials/908/09-fixpass-structure-suite.txt).
The full sweep command was:

```bash
bash -c 'source docker/qemu/lib/gate-structure-preflight.sh; gate_structure_preflight "$PWD" "$BREENIX_GATE_TMP"'
```

It exited 0 with 56/56 suites, 839 tests; no suite outside the UDP-ports
suite failed. A first sweep before the N-6 mutation-premise check also
exited 0 with 56/56 suites, 839 tests. The final sweep output is
`.tmp/908-fixpass-full-sweep.txt`.

The two on-disk mutations ran separately, each followed by
`bash scripts/run-structure-tests.sh udp_ports_lock_irq_structure`:

- N-1: reinserted the removed contended-arm `log::debug!` call. Exit 101,
  16 passed / 2 failed: `handle_udp_contended_arm_has_no_logging` and
  `green_control_all_rules_hold_at_head`. Output:
  `.tmp/908-fixpass-n1-mutation.txt`.
- N-6: deleted the new disclosure only from the primitive's doc comment.
  Exit 101, 16 passed / 2 failed:
  `with_udp_ports_masked_documents_removal_allocator_work` and
  `green_control_all_rules_hold_at_head`. Output:
  `.tmp/908-fixpass-n6-mutation.txt`.

After each mutation, byte-for-byte restoration was verified and the suite
exited 0 with 18/18 tests again (`.tmp/908-fixpass-n1-restored.txt` and
`.tmp/908-fixpass-n6-restored.txt`). The restored kernel diffs contain only
the intended contended-arm change and six added doc-comment lines.

With `TMPDIR="$PWD/.tmp"`, `BREENIX_GATE_TMP="$PWD/.gate-tmp"`, and
`BREENIX_RUST_FORK_LIBRARY=/Users/wrb/fun/code/breenix-parallels/rust-fork/library`,
the rebuild command was:

```bash
cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

It exited 0 with 0 compiler warnings/errors apart from the documented
pre-existing upstream `core` future-incompatibility notice. Output:
`.tmp/908-fixpass-a64-build.txt`. Then
`bash docker/qemu/run-aarch64-boot-test-strict.sh 1` exited 0 with normal
56/56 structure preflight and `PASS: 1/1 boots succeeded`. Output:
`.tmp/908-fixpass-a64-strict-boot.txt`. The fresh serial contains:

```text
[UDP_PORTS_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:driver_cpu=3:irqs_enabled_before=1:masked_in_hold=1:sends=22:hold_us=12020:refused=9:delivered=13:stalled=0:hold_done=1:joined=1:PASS]
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=2:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=12016:netrx_pending_at_release=1:received=12:stalled=0:hold_done=1:joined=1:PASS]
```

The registry marker is named `UDP_PORTS_LOCK_ORACLE`; it carries the
`driver_cpu` and `refused` fields. `UDP_LOCK_ORACLE` is the separate outer
socket marker. The registry observation has holder CPU 1 versus driver
CPU 3, 9 refusals, and the required armed/masked/completion/join fields,
with no stall. This run retains the masked-hold/refusal behavior after
the log removal. Serial:
[09b-fixpass-a64-strict-boot-serial.txt](serials/908/09b-fixpass-a64-strict-boot-serial.txt).
Both new captures were copied byte-for-byte; the existing
`docs/planning/green-program/irq-locks/serials/908/**/*.txt -text` entry
covers them (`git check-attr` reports `text: unset`). Earlier captures
and `.gitattributes` were not edited.

Not claimed: this pass leaves the pre-existing listener-miss arm's
`log::debug!` and `deliver_to_socket`'s `log::warn!` calls unchanged;
they are outside this branch's original added code. No x86 gate was run:
the only executable change is deleting the contended-arm log, and the
fresh aarch64 strict-boot registry oracle still passes. That observation
does not establish x86 runtime behavior. No merge to main is part of
this pass.

Claim discipline for this fix pass:

| invocation | exit code |
|---|---|
| `python3 scripts/claim-lint.py`, first fix-pass check | 1 |
| `python3 scripts/claim-lint.py`, after disclosure wording correction | 0 |

The first lint rejected an unqualified universal quantifier in the suggested disclosure.
The added sentence now says “during alloc/dealloc”; the lint criteria
were not changed.

| invocation (continued) | exit code |
|---|---|
| `python3 scripts/claim-lint.py`, completed evidence draft | 1 |
| `python3 scripts/claim-lint.py --commit-msg .tmp/908-fixpass-commit-message.txt` | 0 |
| `python3 scripts/claim-lint.py`, after evidence wording correction and temporary-artifact relocation | 0 |
| `python3 scripts/claim-lint.py`, after recording that clean tree check | 0 |

The evidence-draft check flagged Rust variant names and the quoted rejected
quantifier in prose, plus generated preflight mutation fixtures under
`.gate-tmp`. The prose now uses descriptive arm names. This pass's generated
preflight directories were moved under `.tmp` for preservation, following
the earlier round's artifact convention; lint criteria were not changed.
