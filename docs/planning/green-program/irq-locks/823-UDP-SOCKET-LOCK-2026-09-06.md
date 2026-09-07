# #823 — the per-socket `Mutex<UdpSocket>` is masked at each of its 4 thread-side holds

Branch `net/823-udp-socket-lock-irq`, off `origin/main` at `6346f2c5` (the
#902 merge). Kernel + oracle + ratchet landed in one commit
(`e3dda6fe`); this document is the second.

## The defect

`kernel/src/net/udp.rs:135` `deliver_to_socket` — the NetRx IRQ route
(`net_rx_softirq_handler` → `handle_udp` → `deliver_to_socket`) — locks the fd
table's outer `Arc<Mutex<UdpSocket>>` at `net/udp.rs:161` (`socket_ref.lock()`),
from inside `with_process_manager` (`net/udp.rs:146`), which masks interrupts
on both architectures.

#812's own document named this exact lock as an open finding it did not
touch: "`net/udp.rs`'s `deliver_to_socket` blocks on the per-socket
`Mutex<UdpSocket>` while `ipc/poll.rs`'s `poll_fd` holds the same lock outside
any mask on the `sys_poll` path. This branch does not touch that lock."

4 thread-side call sites took that same `Mutex<UdpSocket>` without masking
for the whole hold, at `origin/main`:

| site | reached from | shape at `6346f2c5` |
|---|---|---|
| `kernel/src/ipc/poll.rs:110` (`poll_fd`'s `FdKind::UdpSocket` arm) | `sys_poll` directly; `sys_select` via `poll::check_readable`/`check_writable`; `sys_epoll_wait` via `poll::poll_fd` — 3 of 3 UDP-readiness callers funnel through this one function | `socket.lock().has_data()`, no PM guard active, no mask |
| `kernel/src/syscall/socket.rs:292` (`sys_bind`'s `FdKind::UdpSocket` arm) | `sys_bind` | `let mut socket = socket_ref.lock(); socket.bind(..)`, inside `crate::process::manager()` |
| `kernel/src/syscall/socket.rs:448` (`sys_sendto`'s source-port lookup) | `sys_sendto` | `s.lock().local_port().unwrap_or(0)`, inside `manager()` |
| `kernel/src/syscall/socket.rs:612` (`sys_recvfrom`'s nonblocking-flag read) | `sys_recvfrom` | `socket.lock().nonblocking`, inside `manager()` |

The last 3 are inside `crate::process::manager()`'s guard, and `manager()`
masks interrupts on its aarch64 arm (`mrs`/`msr daifset` around
`PROCESS_MANAGER.lock()`) but performs 0 interrupt-state operations on its
non-aarch64 arm (read at `kernel/src/process/mod.rs`, the `pub fn manager()`
definition) — the same aarch64/x86_64 asymmetry #812's own document tabulates
for the same function. `sys_recvfrom`'s OTHER 8 acquisitions of this lock in
the same file already use `Cpu::without_interrupts` explicitly (e.g.
`socket.rs:631`, `Cpu::without_interrupts(|| socket_ref.lock().recv_from())`)
— the nonblocking-flag read at `socket.rs:609` (pre-fix) was the one
inconsistent line in that function.

### Why this is a real same-CPU hazard, not only a contention slowdown

`kernel/src/task/softirqd.rs`'s `do_softirq()` runs at IRQ exit, on top of
whatever was interrupted, before control returns to it. On aarch64,
`handle_irq` (`arch_impl/aarch64/exception.rs`) calls `do_softirq()`
unconditionally after `per_cpu_aarch64::irq_exit()` — #812's own finding is
that aarch64's `in_interrupt()`/`in_softirq()` refusal gate does not read the
preempt-count field, so a preempt-disabled interrupted context does not
refuse it either. If a thread on that CPU is holding the fd table's
`Mutex<UdpSocket>` UNMASKED when the interrupt lands, `do_softirq()` can run
`deliver_to_socket` synchronously, in the same exception, before that thread
is ever resumed — and `deliver_to_socket` tries to lock the exact mutex the
suspended thread already holds. The CPU is suspended in the exception it
took, so it cannot execute the instruction that would release the lock until
the exception returns, and the exception cannot return until the softirq's
own spin on that lock ends. Neither ever happens: permanent wedge. Reproduced
live in the "Red boot" section below.

x86_64 does not reach this specific chain today: `rust_syscall_handler` is
the one dispatch function x86_64 syscalls run through
(`syscall/handler.rs`), and it brackets the syscall body it calls in
`preempt_disable`/`preempt_enable`; separately, `per_cpu::irq_exit()` runs
`do_softirq()` only at `preempt_count() == 0` — the same gate #812 documented
for `try_manager()`.
4 of the 4 call sites above run inside that bracket for their entire hold (the
`poll` re-scan after waking also runs after `preempt_disable()` resumes, at
`syscall/handlers.rs`). This is stated as a present-day reading, not a bound
on future code: #812's own oracle carries the identical caveat for the lock
it tests, and the SMP work tracked separately (`x86-smp-is-a-target.md`) is
an explicit reason not to lean on a single-CPU-shaped argument as a
permanent property. The fix below masks each of the 4 sites unconditionally,
on both architectures, matching the established discipline
(`socket/udp.rs:107-111`'s existing rule for the inner locks, and
`sys_recvfrom`'s own 8 correct sites) rather than relying on
the x86 gate holding.

## The repair

One production primitive, `kernel/src/socket/udp.rs`'s
`with_locked_masked(socket: &Arc<Mutex<UdpSocket>>, f)`:

```rust
pub fn with_locked_masked<F, R>(socket: &Arc<Mutex<UdpSocket>>, f: F) -> R
where
    F: FnOnce(&mut UdpSocket) -> R,
{
    #[cfg(target_arch = "x86_64")]
    type Cpu = crate::arch_impl::x86_64::X86Cpu;
    #[cfg(target_arch = "aarch64")]
    type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;
    use crate::arch_impl::traits::CpuOps;

    Cpu::without_interrupts(|| f(&mut socket.lock()))
}
```

4 of the 4 sites route through it: `poll.rs:118`
(`with_locked_masked(socket, |s| s.has_data())`), `socket.rs:298`
(`with_locked_masked(&socket_ref, |socket| socket.bind(..))`),
`socket.rs:455` (`with_locked_masked(s, |socket| socket.local_port()...)`),
`socket.rs:623` (`with_locked_masked(&socket, |s| s.nonblocking)`). The reads are short;
`sys_bind`'s
ephemeral-port path is bounded but not trivially short: under the registry
lock it takes the nested `next_ephemeral` lock and searches the fixed
ephemeral port range with `BTreeMap::contains_key`, then inserts into the
map (which can allocate); `bind()` also calls `log::debug!` inside the mask.
Masking prevents same-CPU self-reentrancy, so this bounded search does not
introduce that deadlock risk and improves the prior unmasked x86_64 hold.
0 of the 4 sites uses the try-lock-and-defer alternative.

`net/udp.rs`'s IRQ-side counterpart (`deliver_to_socket`'s
`socket_ref.lock()` inside `with_process_manager`) is unchanged.

## The oracle

`udp_socket_lock_oracle`, `kernel/src/test_framework/registry.rs`,
boot_tests-only, registered in the same `TestDef` array as `irq_hold_oracle`
/ `tty_irq_pm_oracle` / `tty_irq_fg_oracle` at `TestStage::ProcessContext`,
for the same reason those three are: it needs a live process row (the socket
the softirq delivers to has to sit in one), and tests within one subsystem
run sequentially on that subsystem's kthread, so this socket's own borrow
cannot overlap the other three oracles' PM/console holds.

The holder calls `with_locked_masked` directly — the same production
function all 4 fixed call sites use — wrapping loopback UDP sends and a
12&nbsp;ms window, on a peer CPU pinned via `kthread_run_on_cpu_for_test`.
Binds a fresh `UdpSocket` on port 54550 into a live process's fd table
(mirroring `irq_hold_open_socket`'s pattern exactly, new port to avoid
colliding with `IRQ_HOLD_PORT` 54540 or `LISTEN_PORT`/`CLIENT_PORT`
54530/54531).

Verdict line, the same 13-field shape `IRQ_HOLD_ORACLE` uses:

```
[UDP_LOCK_ORACLE:aarch64:attempts=A:armed=1:holder_cpu=C:irqs_enabled_before=1:masked_in_hold=1:sends=N:hold_us=H:netrx_pending_at_release=1:received=R:stalled=0:hold_done=1:joined=1:PASS]
```

### The x86 arm

```
[UDP_LOCK_ORACLE:x86:arm=none:reason=irq_exit_gates_softirq_on_preempt_count:online_cpus=1:SKIP]
```

Emitted from `run_udp_lock_oracle_x86_once()` in
`kernel/src/test_framework/executor.rs`, latched with its own
`X823_UDP_LOCK_ORACLE_RAN` `AtomicBool`, called from the same marker-only
stage path as `run_irq_hold_oracle_x86_once()` and the other 2 x86 SKIPs.
Same reason as #812's arm, applied to this lock instead of `PROCESS_MANAGER`:
the oracle's holder is preempt-disabled by construction
(`udp_lock_holder_body`), and `irq_exit()` gates `do_softirq()` on
`preempt_count() == 0`, so the race has no way to fire on this architecture.
SKIP is not a passing result; the aarch64 arm carries the reddens-on-mutation
leg below.

## Red and green

**Red** — this branch with `with_locked_masked`'s own mask removed
(`Cpu::without_interrupts(|| f(&mut socket.lock()))` → `f(&mut socket.lock())`,
the one production primitive the 4 call sites route through, so this is a
single-point mutation of real production code, not a test-only stand-in for
it), `docker/qemu/run-aarch64-boot-test-strict.sh 1` run with
`BREENIX_GATE_SKIP_STRUCTURE=1` (the mutation reddens the new structure
ratchet on its own — see "Mutations" below — so the gate's own preflight
would refuse to boot at all otherwise; this flag exists for exactly this
purpose, per the script's own header comment):

```
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=0:sends=1:hold_us=0:netrx_pending_at_release=0:received=0:stalled=1:hold_done=0:joined=0:FAIL]
```

`masked_in_hold=0` is the defect; `stalled=1:hold_done=0:joined=0` is the
holder kthread that took the lock and never came back — the same signature
#812's own red leg carries for its lock. The boot continues limping (other
CPUs' scheduler/tombstone/TTBR0 censuses keep printing) but never reaches
userspace: `[FAIL] Boot 1: Userspace not detected (640 lines)`, and
`qemu_cpu_s=59.21` against a ~60s window — the CPU that took the lock spun
the rest of the boot away rather than exiting cleanly.
Serial: `serials/823/01-a64-red-unrepaired-serial.txt`.

**Green** — the same tree with the mask restored (this branch, as merged),
`docker/qemu/run-aarch64-boot-test-strict.sh 3`:

```
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=12014:netrx_pending_at_release=1:received=12:stalled=0:hold_done=1:joined=1:PASS]
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=12012:netrx_pending_at_release=1:received=12:stalled=0:hold_done=1:joined=1:PASS]
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=1:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=12014:netrx_pending_at_release=1:received=12:stalled=0:hold_done=1:joined=1:PASS]
```

3/3 boots succeeded (`PASS: 3/3 boots succeeded`). Serials:
`serials/823/02-a64-green-strict-boot1-serial.txt` through
`04-a64-green-strict-boot3-serial.txt`.

A first attempt at this same 3-boot run scored 2/3, with boot 1 producing a
`[FAIL] Boot 1: Userspace not detected (0 lines)` capture —
`load_at_start=7.58`, elevated from other concurrent lanes on this shared
Mac (`feedback-qemu-gate-concurrency.md`) — and 0 bytes of serial captured at
all, unlike every red/green boot in this round, all of which captured
hundreds to tens of thousands of lines. Not counted as a reading: it is a
host-launch capture failure with no kernel content to score, not a boot
that ran and failed. The 3/3 run above was taken after host load settled
(re-run, same kernel, same command).

## Gate pins

Mirrors the `IRQ_HOLD_ORACLE` pins from #812, `UDP_LOCK_ORACLE`-named, in the
4 gate scripts:

| gate | pin |
|---|---|
| `run-aarch64-boot-test-strict.sh` | `UDP_LOCK_ORACLE_PATTERN` in `score_serial`, plus a FAIL scan; `'[UDP_LOCK_ORACLE:'` added to the `require_boot_tests_kernel` marker census; a self-check block that verifies the pattern rejects `hold_us=0` and accepts `hold_us=12034` before it scores any boot |
| `run-aarch64-prod-profile-boot-test.sh` | `UDP_LOCK_ORACLE_LITERAL` asserted at count 0, and the count printed (both the early per-boot line and the final summary) |
| `run-x86-boot-tests.sh` | the x86 SKIP literal added to the pass-chain `&&` sequence and to the "extract and echo the matched line" block |
| `run-x86-prod-profile-boot-test.sh` | `'[UDP_LOCK_ORACLE:'` added to the forbidden test-only-marker census |

### Gate-side mutations of the pins

Anti-vacuity for the strict-gate self-check, run inline as part of the gate
script itself: `udp_lock_oracle_sample 0` (i.e. `hold_us=0`) is asserted NOT
to match `UDP_LOCK_ORACLE_PATTERN`, and `udp_lock_oracle_sample 12034`
(`hold_us=12034`) is asserted TO match it — both checked before the gate
scores any boot, so a pattern that regressed to accept a zero-length hold
(a window no timer tick could land in) would fail the gate outright rather
than silently score green. This block ran as part of each of the 4
strict-gate invocations in this round (3 green boots, 1 red boot, captured
above); 0 of the 4 hit either self-check failure message.

## The ratchet

`tests/udp_socket_lock_irq_structure.rs`, 12 tests, 6 shape-check rules each
paired with its own anti-vacuity mutation (`replacen` on the real,
in-memory source text, asserted to have actually matched before the mutated
text is re-scanned):

1. `with_locked_masked_masks_the_whole_hold` / `..._rule_is_not_vacuous` — the
   primitive's body contains `without_interrupts` and runs `f()` under the
   acquired lock.
2. `poll_fd_udp_arm_uses_the_masked_primitive` / `..._rule_is_not_vacuous` —
   `poll_fd`'s `UdpSocket` arm calls the primitive and does not also take a
   raw `socket.lock()`.
3. `sys_bind_udp_arm_uses_the_masked_primitive` / `..._rule_is_not_vacuous` —
   `sys_bind`'s `UdpSocket` arm calls the primitive around `bind(..)`.
4. `sys_sendto_udp_arm_uses_the_masked_primitive` / `..._rule_is_not_vacuous`
   — `sys_sendto`'s source-port read calls the primitive.
5. `sys_recvfrom_nonblocking_read_uses_the_masked_primitive` /
   `..._rule_is_not_vacuous` — `sys_recvfrom`'s nonblocking-flag read calls
   the primitive.
6. `deliver_to_socket_still_locks_under_process_manager` — a regression
   guard, not new-defect pinning: `deliver_to_socket` still runs inside
   `with_process_manager` and still locks `socket_ref`, so the IRQ side's own
   masking cannot be silently moved out from under it while the thread side
   is being kept honest.

Plus `green_control_all_rules_hold_at_head`, which calls each of the 6 rule
functions in sequence so a run of just this file with 0 failures is itself
evidence.

`code_mask`, copied verbatim from `tests/tty_irq_pm_structure.rs`, strips
comments and string/char literals byte-for-byte before any of the 6 rules
search the text, so a substring match cannot be satisfied from inside a
comment.

**Stated limit on rule 6.** Its check that `deliver_to_socket` runs "inside"
`with_process_manager` is a whole-file substring search bounded by the
function's own start and the next `\nfn `, not a nesting-aware parse; at this
head `with_process_manager` occurs exactly once in `net/udp.rs` (line 146,
inside `deliver_to_socket`, which is also the last function in the file), so
the rule is not currently satisfiable by an occurrence anywhere else in the
file. It has no dedicated anti-vacuity mutation test of its own (rules 1-5
each do); this is disclosed, not fixed here, because tightening it further
is a bigger change to the shared `code_mask`-based scanning approach than
this round's scope, and the current shape already reddens if the last
remaining `with_process_manager` occurrence is removed from the file —
which is exactly what a change that stopped masking the IRQ-side lock would
do.

## Fixture re-record (the R182 standing step)

The strict gate's `require_boot_tests_kernel` marker census now requires
`'[UDP_LOCK_ORACLE:'`, which means any other suite's committed
`GREEN_SERIAL` fixture that gets scored through `score_with_gate` against
the strict gate now needs that marker present too, or the gate itself
refuses the kernel before it ever reaches that suite's own assertions — the
same standing step #822's document describes for its own oracle's addition.

4 suites were affected: `tests/tty_irq_pm_structure.rs`,
`tests/tty_irq_fg_structure.rs`, `tests/loopback_pump_structure.rs`,
`tests/ttbr0_shadow_reconciliation_structure.rs`. All 4 now point at a single
new committed fixture, `tests/fixtures/udp-socket-lock-aarch64-serial.txt` —
one real strict-gate boot at this branch's head, which carries every marker
those 4 suites' own ratchets require (`TTY_IRQ_PM_ORACLE`, `TTY_IRQ_FG_ORACLE`,
`PIN_GUARD_ORACLE`, `RING_SPAN`) alongside `UDP_LOCK_ORACLE`, all reading
`PASS`. `.gitattributes` gained a `-text` entry for this fixture and for
`docs/planning/green-program/irq-locks/serials/823/**/*.txt`, matching every
other raw-serial-capture directory in that file — the CR bytes the guest
console emits are part of what these suites read, and a CRLF normalisation
on commit would edit them.

## Gates run

| gate | result | evidence |
|---|---|---|
| aarch64 build, `--features boot_tests` | exit 0, 0 kernel warnings/errors (1 pre-existing upstream `core` future-incompatibility notice, confirmed present on unmodified `origin/main` at `5f68fedd` with the identical command — not this branch's) | `/tmp/823-arm-final.log`, not committed |
| x86_64 build, `--features testing,external_test_bins --bin qemu-uefi` (Mac, native) | exit 0, 0 warnings/errors | — |
| x86_64 build, `--features boot_tests,testing,external_test_bins --bin qemu-uefi` (beast, fresh clone) | exit 0, 0 `^(warning\|error)` lines | `serials/823/07-x86-build.txt` |
| every `tests/*_structure.rs` suite | 53/53 green, 786 tests | independently re-run from this session, `/tmp/823-full-sweep-independent.log`, not committed |
| `run-aarch64-boot-test-strict.sh 3` | 3/3 PASS | `serials/823/02` – `04` |
| `run-aarch64-prod-profile-boot-test.sh` | PASS, `Observed UDP-socket-lock oracle marker count: 0` | `serials/823/05-a64-prod-profile-gate.txt` |
| `run-x86-boot-tests.sh 1` on beast | `x86 frame-custody gate run 1: PASS`, `EXIT_CODE:0`; `x86 userspace gate: PASS - exited=110 expected>=105 nonzero=0 allowlist=0` | `serials/823/06-x86-boot-tests-gate.txt`, serial excerpt `06b` |
| `run-x86-prod-profile-boot-test.sh` on beast | PASS on retry (see below); `test-only marker '[UDP_LOCK_ORACLE:': 0` | `serials/823/08-x86-prod-profile-gate.txt` |
| `scripts/claim-lint.py` | exit 0 (clean, 27 files checked across the full branch, changed hunks vs merge base `6346f2c53817`, including this review-fix commit) | — |
| `scripts/claim-lint.py --commit-msg` | exit 0 | — |

**The x86 prod-profile gate's first attempt scored FAIL**, on
`PROMPT_LIVENESS_ENDED_BY=prompt_absent` (`before=0 after=1` — the
steady-state console prompt printed once inside the liveness-check window,
not the required twice). The gate script's own failure message names this
class explicitly, citing #826: "a starved guest that reaches steady state
late in its own sampling window, or that is too starved to answer the
liveness stimulus inside the window, produces exactly this shape with no
kernel regression involved" — and separately confirms crash markers and the
teardown census, which this failure mode does not skip, were clean. At the
time of that attempt, `ps aux` on the beast VM showed a concurrent lane's
`teardown_structure` structure-test binary consuming 300-450% CPU and a
second lane's QEMU boot active simultaneously. A retry after those finished
(host load average dropped from ~7-8 to 0.12) scored PASS cleanly on the
first attempt, with `console prompt count over 60s: 1 -> 2` — the liveness
stimulus answered as designed. Both logs are preserved:
`serials/823/09-x86-prod-profile-gate-host-contention-flake.txt` (the FAIL)
and `serials/823/08-x86-prod-profile-gate.txt` (the PASS retry, the one
counted in the table above).

## Claim discipline

```
claim-lint: scripts/claim-lint.py                                   -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <commit msg file>    -> exit 0
```

Both run from this branch's working tree before each commit in this round.

## Not claimed

* **The `udp_ports` registry lock.** While tracing each acquisition of the
  fd table's `Mutex<UdpSocket>`, a SEPARATE lock with the same unmasked-hold
  shape was found: `SOCKET_REGISTRY.udp_ports` (`kernel/src/socket/mod.rs`),
  locked
  unmasked by `UdpSocket::Drop` (`unbind_udp`) and read unmasked by
  `handle_udp`'s `lookup_udp` call, which runs in the NetRx IRQ route BEFORE
  `deliver_to_socket`'s `with_process_manager` scope begins. The normal
  process-termination path (`Process::terminate` → `close_all_fds`) is
  reached only through `with_process_manager`, which masks on both
  architectures, so that path is not exposed. A SEPARATE ordinary thread-exit
  path, `kernel/src/task/process_task.rs`'s `close_extracted_fds`, is called
  by `handle_thread_exit`. Its production callers are
  `kernel/src/arch_impl/aarch64/syscall_entry.rs`,
  `kernel/src/syscall/handlers.rs`, `kernel/src/process/mod.rs`'s
  `exit_process_and_retire`, and the self-triggered fault-exit call in
  `process_task.rs`. This is not the P6a deferred-reclaim/tombstone path:
  `reclaim_deferred_process_resources` and
  `reclaim_deferred_process_resources_for_pass` call neither exit helper.
  The former does independently have callers in both
  `kernel/src/interrupts/context_switch.rs` and
  `kernel/src/arch_impl/aarch64/context_switch.rs`.
  `close_extracted_fds` is documented as running "No PM lock is held when
  this runs", does not appear to mask interrupts, and has no
  `FdKind::UdpSocket` match arm to protect the unmasked `UdpSocket::Drop`.
  This is a plausible instance of the same defect class on a different lock
  in ordinary thread-exit cleanup, for which this round did not build an
  oracle. It is not fixed here. Filed as #908 rather than left undisclosed.
* **No `try-lock-and-defer` site.** The brief allowed either masking or
  try-lock-and-defer per site; the 4 holds in this round were short enough
  that masking the whole hold was the natural choice at each one, and 0 of
  the 4 uses try-lock-and-defer.
* **x86_64's protection is a chain property of `rust_syscall_handler`'s
  preempt-disable bracket, not a masking property of the fix itself.** The
  fix masks unconditionally on both architectures regardless, but the "Why
  this is a real same-CPU hazard" section's x86 claim rests on that bracket
  holding across future changes to syscall dispatch, which this round did
  not audit end-to-end.
* **`hold_us` timing values are readings from this Mac's clock, not a
  cross-machine bound**, the same caveat #812's and #822's documents carry
  for their own `hold_us`/`entry_us` fields.


## Fix pass — review findings closed (2026-09-06)

The independent review combined a direct code census with a second Codex
pass, recorded in the coordinator's records. It raised 5 findings and gave
1 "confirmed clean" verdict covering the remainder of the reviewed change:

1. Minor/code: this branch's own `udp_lock_received` checker held the
   socket mutex unmasked. It now calls the production
   `crate::socket::udp::with_locked_masked` primitive around the receive
   queue inspection, with a comment identifying the review gap.
2. Minor/code, doc-only correction: the repair prose understated the
   ephemeral-bind masked hold. It now describes the nested lock, bounded
   port-range search, map insertion/allocation, and debug call.
3. Minor/prose, doc-only caller-chain correction: `close_extracted_fds`
   belongs to `handle_thread_exit`'s ordinary exit cleanup, not the P6a
   deferred-reclaim path. The production callers and the separate
   context-switch reclaim callers were rechecked against the source.
4. Minor/prose: the claim-lint table now reports the full-branch file count
   and merge base, rather than the first implementation commit's count.
5. Nit/prose: the x86 host-contention failure now says `before=0 after=1`:
   the prompt printed once during the window, not the required twice.

Rule 7 in `tests/udp_socket_lock_irq_structure.rs` pins the checker's
primitive call in its function body after `code_mask` stripping. Its
anti-vacuity test uses `replacen` on the real source to restore the old
unmasked function in memory, asserts the replacement matched, and checks
that the primitive call disappeared from the mutated body. Rules 1–6 were
not restructured. The green control also calls rule 7.

Verification in this pass: `scripts/run-structure-tests.sh
udp_socket_lock_irq_structure` passed 14/14 tests, including rule 7's
mutation. The full `tests/*_structure.rs` sweep passed 53/53 suites,
788 tests. An on-disk rule-1 mutation replacing
`Cpu::without_interrupts(|| f(&mut socket.lock()))` with
`f(&mut socket.lock())` exited 101: both
`with_locked_masked_masks_the_whole_hold` and
`green_control_all_rules_hold_at_head` failed as required. After restoring
`kernel/src/socket/udp.rs` byte-for-byte, the suite passed 14/14 again.

The pre-existing #812 checker, `irq_hold_received` in
`kernel/src/test_framework/registry.rs`, has the identical unmasked-lock
shape (the same lock/queue/count body, with its own payload constant).
`git show main:kernel/src/test_framework/registry.rs` confirms it already
exists on main (definition at line 5249 in that snapshot). It predates this
branch and is explicitly untouched here, out of scope for this fix pass
and left for a separate issue/branch by the coordinator; this pass files
no issue for it.


The aarch64 `boot_tests` release build exited 0 with 0 kernel warnings or
errors; the expected pre-existing upstream `core` future-incompatibility
notice was the only warning. `bash docker/qemu/run-aarch64-boot-test-strict.sh
1` exited 0 and reported `PASS: 1/1 boots succeeded`. Its serial contains:

```
[UDP_LOCK_ORACLE:aarch64:attempts=1:armed=1:holder_cpu=2:irqs_enabled_before=1:masked_in_hold=1:sends=12:hold_us=12018:netrx_pending_at_release=1:received=12:stalled=0:hold_done=1:joined=1:PASS]
```

Serial: `serials/823/10-fixpass-a64-strict-boot-serial.txt`, copied without
text normalization; the existing `.gitattributes` entry covers it.
