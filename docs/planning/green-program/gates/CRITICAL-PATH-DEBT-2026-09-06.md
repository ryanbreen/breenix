<!--
Filed verbatim from the working plan authored against `origin/main`
783a6a53668d99225e356530a124c081d1fdcbd3 (2026-09-06), as PR-0's deliverable
(4) in `docs/planning/green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md`.
The body below is unedited; PR-0 itself (the census ratchet,
`tests/critical_path_logging_census_structure.rs`, and the shell-script
pattern widening described in this doc's own §1 and PR-0 section) is
recorded separately in `docs/planning/green-program/gates/
CRITICAL-PATH-DEBT-PR0-ROUND-2026-09-06.md`. Re-derivation note: this
session independently ran `bash scripts/check-critical-path-violations.sh`
against the same commit and got the identical 274-stdout-line, exit-1
result this doc's own header cites, and a Rust re-implementation of the
same census (in the structure suite named above) reproduced this doc's
135-site / 9-file total and its per-file breakdown exactly -- see the round
doc for the reproduction command and output.
-->

# Critical-path logging: checker-debt classification

**Snapshot:** `origin/main` `783a6a53668d99225e356530a124c081d1fdcbd3` (`783a6a53`, "Merge pull request #878 from ryanbreen/fix/821-tty-irq-no-pm-block") — 2026-09-06 07:35 ET
**Producer:** `bash scripts/check-critical-path-violations.sh` — exit 1, 274 stdout lines, 9 `VIOLATION in <file>` headers.
**Read-only census.** No repository file was modified to produce this document.

---

## 1. What the script is, exactly

`scripts/check-critical-path-violations.sh` greps a fixed list of kernel sources for a fixed list of
spellings and exits 1 if any line matches.

**The file list** (`CRITICAL_FILES`, :29-62) is 12 named paths plus one directory entry:

| Entry | Flagged today |
|---|---|
| `arch_impl/aarch64/context_switch.rs` | 6 |
| `arch_impl/aarch64/context.rs` | 0 |
| `interrupts/context_switch.rs` | 30 |
| `arch_impl/aarch64/timer_interrupt.rs` | 18 |
| `arch_impl/aarch64/exception.rs` | 9 |
| `interrupts/timer.rs` | 0 |
| `interrupts/timer_entry.asm` | 0 (it is grepped, but only Rust-macro spellings are searched for, so an assembly file cannot match) |
| `syscall/handler.rs` | 2 |
| `syscall/entry.asm` | 0 (same) |
| `syscall/time.rs` | 1 |
| `per_cpu.rs` | 12 |
| `per_cpu_aarch64.rs` | 6 |
| `arch_impl/aarch64/percpu.rs` | 0 |
| `task/scheduler.rs` | 51 |
| `capture/` (directory, 4 `.rs` files enumerated from disk) | 0 |

**The shared denylist** (`PROHIBITED_PATTERNS`, :102-116): `serial_println!`, `log::debug!`,
`log::info!`, `log::warn!`, `log::error!`, `log::trace!`, `println!`, `eprintln!`, `format!`,
`write!`, `writeln!`, `crate::serial_println!`.

**A capture-only extra denylist** (`CAPTURE_PROHIBITED_PATTERNS`, :87-99) adds `\.lock()`,
`try_dump_state`, `alloc::`, `Vec<`, `String`, `Box<`, `vec!`, `to_string`, `unwrap()`, `expect(`,
`panic!` — applied only under `kernel/src/capture/`.

**What it accepts:** 0 spellings are listed positively. The comment block at :118-123 names
`raw_uart_char()`/`raw_uart_str()`, `trace_event()`/`trace_*!`, and
`raw_serial_char()`/`raw_serial_str()` as the sanctioned alternatives, but that is prose — those
spellings are accepted only because they are absent from the denylist.

**Allowlist / annotation mechanism: the script has 0 of them.** No per-line pragma, no per-site exception file,
no `# critical-path:ok` comment form. The only escape the script offers is the comment filter
`grep -v "^[^:]*:[[:space:]]*//"` (:149), which drops a line whose first non-whitespace content is `//`.
That is why the script has been red on `main` with 0 partial credit and is not wired into any gate: the
only references to it in the tree are `tests/capture_path_lock_free_structure.rs` (which keeps the
capture denylist in step) and round docs that record it as "exit 1, unchanged from main".

**Two reporting defects worth naming while we are here:**

1. *Report inflation.* `println!` is a substring of `serial_println!` and of `log_serial_println!`,
   and `crate::serial_println!` contains `serial_println!`. Each
   `crate::serial_println!` line is therefore printed three times and each `log_serial_println!`
   line twice. That is how 135 distinct source lines become 274 stdout lines. The `VIOLATION in`
   header is also printed *after* the grep output it heads, because the `echo` runs inside the
   `if grep ...; then` body.
2. *A spelling that escapes.* `serial_print!` (no `ln`) matches 0 of the 12 patterns, yet routes to
   the same `_print()` → blocking `SERIAL1.lock()`. There is exactly one such site inside the
   checked files today: **`kernel/src/arch_impl/aarch64/exception.rs:1985`**,
   `crate::serial_print!("{}", byte as char);` in `fn sys_write`, one blocking serial acquisition
   *per byte written*. `log_serial_print!` and `log::log!` escape the same way; neither appears in
   a checked file today. This site is not in the 135 and is carried separately below.

---

## 2. What the flagged calls actually do

Reading the call chain matters more than the macro name, because three of the four macro families
behave differently:

| Spelling | x86_64 | aarch64 |
|---|---|---|
| `serial_println!` | `serial::_print` → `SERIAL1.lock()` (COM1, blocking) | `serial_aarch64::_print` → mask DAIF, `SERIAL1.lock()` (PL011, blocking) |
| `log_serial_println!` | `serial::_log_print` → disable IRQs, `SERIAL2.lock()` (COM2, blocking), `.expect(...)` | `serial_aarch64::_log_print` → `_print` → the **same** `SERIAL1` mutex |
| `log::{info,warn,error,debug}!` | `CombinedLogger::log` → `state.try_lock()` → `log_serial_println!` → `SERIAL2` | same logger → `SERIAL1` |
| `log::trace!` | `CombinedLogger::log` **returns before taking any lock** (`kernel/src/logger.rs`, the `Level::Trace` early return) | same |

`log::set_max_level(LevelFilter::Trace)` (`kernel/src/logger.rs:1104`) means 0 records are filtered at
the `log` crate's static gate; the Trace suppression is the logger's own early return. So a
`log::trace!` in a critical file **emits 0 bytes and takes 0 locks today** — it still evaluates its
format arguments, and it is one logger change away from being a live acquisition.

`kernel/src/task/scheduler.rs:5-35` states the ordering the tree relies on:

> Level 1: SCHEDULER · Level 2: PROCESS_MANAGER · Level 3: STDIN_BUFFER/BLOCKED_READERS · Level 4: SERIAL1
>
> **Never acquire SERIAL1 while holding SCHEDULER or PROCESS_MANAGER.** This means no
> `serial_println!`, `log_serial_println!`, or `write_byte()` calls from code that holds the
> scheduler lock.
> claim-lint:ok: that absolute is quoted verbatim from `kernel/src/task/scheduler.rs:20-25`, not asserted here.

That bullet names `log_serial_println!` explicitly and without an architecture qualifier. The
rationale paragraph below it is aarch64-specific (one PL011, one lock), and the x86 carve-out is
stated inline at `scheduler.rs:2151-2154`: COM2 is a different mutex from COM1. Both things are
true; the rule as written is broader than the rationale that justifies it, and that gap is where
most of the H1 rows below sit.

---

## 3. Hazard classes used in the table

- **H1 — can deadlock or tear.** A blocking serial lock is acquired at a point where SCHEDULER or
  PROCESS_MANAGER is provably held, and/or in interrupt context on a locked writer. This is the
  ordering `scheduler.rs:21-22` forbids. It does **not** claim an observed deadlock: no AB-BA cycle
  through `SERIAL2` exists in this tree today, because a grep over `kernel/src` finds 0 sites that take PM or SCHEDULER while holding a
  serial mutex, and `_log_print`/`_print` mask interrupts around the acquisition. What it does claim
  is the shape, plus two measurable costs: the lock hold is extended across UART output the file's
  own comment prices at "~960 bytes/sec … 50-100ms" per line (`scheduler.rs:2143-2144`), and
  `_log_print`'s `.expect(...)` panics while holding `SERIAL2`.
- **H2 — timing hazard on a hot path, even when gated.** Interrupt-return path or syscall bracket,
  no lock provably held at the print.
- **H3 — cold path in a critical file: rule debt, no runtime hazard identified.** Init-time,
  thread-context oracle emitters, dead code, unreachable-in-production arms.
- **H4 — the script misreads it.** The line cannot execute in any shipped kernel.

Counts: **H1 43 · H2 9 · H3 75 · H4 8** across **135 lines / 9 files**.

---

## 4. The table

Each row is one line the script flags. `fn` is the innermost enclosing function.


### `kernel/src/interrupts/context_switch.rs` — 30 lines (H1 16 · H2 5 · H3 9)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:458` | `log::error!` | `check_need_resched_and_switch` | IRQ-return dispatch path, PROCESS_MANAGER guard held (acquired :347, moved only at :507) | ungated | not gate-pinned | yes - trace_dispatch_abandon(DispatchAbandonSite::RollbackSaveFailed) on the very next line (:465) | **H1** |  |
| `:557` | `log::trace!` | `check_need_resched_and_switch` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:642` | `log::trace!` | `save_current_thread_context_with_guard` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:649` | `log::error!` | `save_current_thread_context_with_guard` | IRQ-return dispatch path, inside save_current_thread_context_with_guard(manager_guard: &mut TryProcessManagerGuard) | ungated | not gate-pinned | partly - dispatch_strand_census::note_save covers the success arm only; these three failure arms have no counter | **H1** | Failure arms need a new per-arm counter before the print can go. |
| `:656` | `log::error!` | `save_current_thread_context_with_guard` | IRQ-return dispatch path, inside save_current_thread_context_with_guard(manager_guard: &mut TryProcessManagerGuard) | ungated | not gate-pinned | partly - dispatch_strand_census::note_save covers the success arm only; these three failure arms have no counter | **H1** | Failure arms need a new per-arm counter before the print can go. |
| `:662` | `log::error!` | `save_current_thread_context_with_guard` | IRQ-return dispatch path, inside save_current_thread_context_with_guard(manager_guard: &mut TryProcessManagerGuard) | ungated | not gate-pinned | partly - dispatch_strand_census::note_save covers the success arm only; these three failure arms have no counter | **H1** | Failure arms need a new per-arm counter before the print can go. |
| `:748` | `log::trace!` | `save_kthread_context` | IRQ-return dispatch path, INSIDE a scheduler::with_thread_mut closure (SCHEDULER held); Trace records are dropped before the lock is taken | ungated | not gate-pinned | n/a | **H3** | ARMED TRIPWIRE: raising the trace filter or swapping the logger turns this into SCHEDULER -> SERIAL2 in IRQ context. |
| `:821` | `log::trace!` | `switch_to_thread` | IRQ-return dispatch path, INSIDE a scheduler::with_thread_mut closure (SCHEDULER held); Trace records are dropped before the lock is taken | ungated | not gate-pinned | n/a | **H3** | ARMED TRIPWIRE: raising the trace filter or swapping the logger turns this into SCHEDULER -> SERIAL2 in IRQ context. |
| `:837` | `log::error!` | `switch_to_thread` | IRQ-return dispatch path, switch_to_thread still owns process_manager_guard (:807, moved at :920/:1214) | ungated | not gate-pinned | yes - trace_dispatch_abandon(DispatchAbandonSite::RollbackTls) at :839 | **H1** |  |
| `:882` | `log::trace!` | `switch_to_thread` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:980` | `log::info!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | partial - the terminated arm has DispatchAbandonSite::IdleSignalTerminatedBlocked (:1104); the delivery arms have 0 | **H1** |  |
| `:1030` | `log::info!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | partial - the terminated arm has DispatchAbandonSite::IdleSignalTerminatedBlocked (:1104); the delivery arms have 0 | **H1** |  |
| `:1079` | `log::info!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | partial - the terminated arm has DispatchAbandonSite::IdleSignalTerminatedBlocked (:1104); the delivery arms have 0 | **H1** |  |
| `:1092` | `log::debug!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | not yet | **H1** | log::debug! - emits (only Trace is suppressed by CombinedLogger). |
| `:1111` | `log::info!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | partial - the terminated arm has DispatchAbandonSite::IdleSignalTerminatedBlocked (:1104); the delivery arms have 0 | **H1** |  |
| `:1194` | `log::error!` | `switch_to_thread` | IRQ-return dispatch path, inside the manager_guard binding opened at :921 | ungated | not gate-pinned | yes - trace_dispatch_abandon(DispatchAbandonSite::RollbackKernelContextLock) at :1200 | **H1** |  |
| `:1232` | `log::error!` | `setup_idle_return` | IRQ-return dispatch path; SCHEDULER released before the unwrap_or_else fires, but 11 of setup_idle_return's 13 callers sit in this file - 4 in switch_to_thread, 5 in restore_userspace_thread_context, 2 in check_and_deliver_signals_for_current_thread - and each of those 11 is inside a live PROCESS_MANAGER guard scope | ungated | not gate-pinned | not yet | **H1** | Classified H1 on the caller-held PM guard, not on SCHEDULER. |
| `:1262` | `log::trace!` | `setup_idle_return` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:1337` | `log::error!` | `setup_kernel_thread_return` | IRQ-return dispatch path; setup_kernel_thread_return is called from switch_to_thread while it still owns the PM guard | ungated | not gate-pinned | not yet | **H1** |  |
| `:1352` | `log::trace!` | `restore_userspace_thread_context` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:1369` | `log::info!` | `restore_userspace_thread_context` | IRQ-return dispatch path, no lock held (PM guard moved into setup_first_userspace_entry at :1366) | ungated | not gate-pinned | not yet | **H2** | Fires once per thread, not once per boot. |
| `:1392` | `log::trace!` | `restore_userspace_thread_context` | IRQ-return dispatch path; Trace records are dropped by CombinedLogger::log before any lock is taken (kernel/src/logger.rs, the Level::Trace early return) | ungated | not gate-pinned | n/a | **H3** | Emits 0 bytes today; it still evaluates its format arguments on each dispatch. |
| `:1429` | `log::error!` | `restore_userspace_thread_context` | IRQ-return dispatch path, inside the manager_guard binding opened at :1402 | ungated | not gate-pinned | yes for :1429 - DispatchAbandonSite::IdleRestoreError at :1445 | **H1** |  |
| `:1497` | `log::trace!` | `restore_userspace_thread_context` | IRQ-return dispatch path, INSIDE a scheduler::with_thread_mut closure (SCHEDULER held); Trace records are dropped before the lock is taken | ungated | not gate-pinned | n/a | **H3** | ARMED TRIPWIRE: raising the trace filter or swapping the logger turns this into SCHEDULER -> SERIAL2 in IRQ context. |
| `:1499` | `log::error!` | `restore_userspace_thread_context` | IRQ-return dispatch path, inside the manager_guard binding opened at :1402 | ungated | not gate-pinned | yes for :1429 - DispatchAbandonSite::IdleRestoreError at :1445 | **H1** |  |
| `:1512` | `log::debug!` | `restore_userspace_thread_context` | IRQ-return dispatch path, inside the manager_guard binding opened at :1402 | ungated | not gate-pinned | not yet | **H1** | log::debug! - emits. |
| `:1582` | `log::error!` | `restore_userspace_thread_context` | IRQ-return dispatch path, PM lock NOT acquired (this is the else arm) | ungated | not gate-pinned | not yet - note_dispatch_guard_unavailable() exists at :121 but is called only from :353 | **H2** |  |
| `:1683` | `log::info!` | `setup_first_userspace_entry` | IRQ-return dispatch path, after drop(manager_guard) at :1623 | ungated | not gate-pinned ("RING3_ENTRY: Thread entering Ring 3", the string tests/ring3_smoke_test.rs:26 looks for, has no producer in kernel/src) | not yet | **H2** |  |
| `:1712` | `log::info!` | `setup_first_userspace_entry` | IRQ-return dispatch path, after drop(manager_guard) at :1623 | ungated | not gate-pinned ("RING3_ENTRY: Thread entering Ring 3", the string tests/ring3_smoke_test.rs:26 looks for, has no producer in kernel/src) | not yet | **H2** |  |
| `:1714` | `log::info!` | `setup_first_userspace_entry` | IRQ-return dispatch path, after drop(manager_guard) at :1623 | ungated | not gate-pinned ("RING3_ENTRY: Thread entering Ring 3", the string tests/ring3_smoke_test.rs:26 looks for, has no producer in kernel/src) | not yet | **H2** |  |

### `kernel/src/task/scheduler.rs` — 51 lines (H1 27 · H3 16 · H4 8)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:275` | `crate::serial_println!` | `emit_wake_attribution_counters` | thread-context census read-out - emit_wake_attribution_counters, single caller net/mod.rs:719 (kloopbackd) | cfg-gated on not(target_arch = aarch64) nearby at :270 | not gate-pinned | yes - prints only the WAKE_SITE_* / ENQUEUE_* atomics | **H3** |  |
| `:285` | `crate::serial_println!` | `emit_wake_attribution_counters` | thread-context census read-out - emit_wake_attribution_counters, single caller net/mod.rs:719 (kloopbackd) | cfg-gated on not(target_arch = aarch64) nearby at :270 | not gate-pinned | yes - prints only the WAKE_SITE_* / ENQUEUE_* atomics | **H3** |  |
| `:490` | `crate::serial_println!` | `emit_pinned_placement_census` | thread-context census read-out - emit_pinned_placement_census, callers main_aarch64.rs:1366 and strand_oracle.rs:478 | ungated | docker/qemu/run-aarch64-{prod-profile-boot-test,boot-test-strict}.sh; tests/loopback_pump_structure.rs (marker PINNED_HOME_CPU_UNAVAILABLE) | yes - PINNED_* atomics | **H3** |  |
| `:531` | `crate::serial_println!` | `emit_pin_guard_oracle` | boot-thread oracle print - emit_pin_guard_oracle, callers main.rs:731 and main_aarch64.rs:1365 | cfg-gated on target_arch AND feature = boot_tests, on both arms (:529, :537) | docker/qemu/run-aarch64-{boot-test-strict,prod-profile-boot-test}.sh; tests/loopback_pump_structure.rs (marker PIN_GUARD_ORACLE) | partly | **H3** |  |
| `:540` | `crate::serial_println!` | `emit_pin_guard_oracle` | boot-thread oracle print - emit_pin_guard_oracle, callers main.rs:731 and main_aarch64.rs:1365 | cfg-gated on target_arch AND feature = boot_tests, on both arms (:529, :537) | docker/qemu/run-aarch64-{boot-test-strict,prod-profile-boot-test}.sh; tests/loopback_pump_structure.rs (marker PIN_GUARD_ORACLE) | partly | **H3** |  |
| `:569` | `crate::serial_println!` | `emit_pin_guard_oracle` | boot-thread oracle print - emit_pin_guard_oracle, callers main.rs:731 and main_aarch64.rs:1365 | cfg-gated on target_arch AND feature = boot_tests, on both arms (:529, :537) | docker/qemu/run-aarch64-{boot-test-strict,prod-profile-boot-test}.sh; tests/loopback_pump_structure.rs (marker PIN_GUARD_ORACLE) | partly | **H3** |  |
| `:578` | `crate::serial_println!` | `emit_pin_guard_oracle` | boot-thread oracle print - emit_pin_guard_oracle, callers main.rs:731 and main_aarch64.rs:1365 | cfg-gated on target_arch AND feature = boot_tests, on both arms (:529, :537) | docker/qemu/run-aarch64-{boot-test-strict,prod-profile-boot-test}.sh; tests/loopback_pump_structure.rs (marker PIN_GUARD_ORACLE) | partly | **H3** |  |
| `:1981` | `serial_println!` | `add_thread_inner` | SCHEDULER HELD - add_thread_inner is an impl Scheduler method (impl spans :1732-:5175) | cfg-gated on target_arch = x86_64 at :1980 | not gate-pinned | not yet | **H1** |  |
| `:2091` | `serial_println!` | `add_thread_as_current` | SCHEDULER HELD - add_thread_as_current, impl Scheduler | cfg-gated on target_arch = x86_64 at :2090 | not gate-pinned | not yet | **H1** |  |
| `:2341` | `serial_println!` | `schedule` | SCHEDULER HELD - inside Scheduler::schedule(&mut self) (:2149), the dispatch decision itself | runtime gate: if debug_log, where debug_log = _count < 5 \|\| _count % 500 == 0 on x86_64 and literal false elsewhere (:2155-2157) | not gate-pinned | partly - SCHEDULE_COUNT (:2145) and context_switch_count() already exist | **H1** | THE CALL-OUT: :2341 contradicts this file's own Key Rule at :21-22, which names log_serial_println! by name as forbidden under the scheduler lock. The x86 carve-out at :2151-2154 is that COM2 is a different mutex from COM1 - true, and narrower than the rule as written. The file's own comment at :2143-2144 prices the line: "Serial output is ~960 bytes/sec, so each log line can take 50-100ms". |
| `:2447` | `serial_println!` | `schedule` | SCHEDULER HELD - inside Scheduler::schedule(&mut self) (:2149), the dispatch decision itself | runtime gate: if debug_log, where debug_log = _count < 5 \|\| _count % 500 == 0 on x86_64 and literal false elsewhere (:2155-2157) | not gate-pinned | partly - SCHEDULE_COUNT (:2145) and context_switch_count() already exist | **H1** | THE CALL-OUT: :2341 contradicts this file's own Key Rule at :21-22, which names log_serial_println! by name as forbidden under the scheduler lock. The x86 carve-out at :2151-2154 is that COM2 is a different mutex from COM1 - true, and narrower than the rule as written. The file's own comment at :2143-2144 prices the line: "Serial output is ~960 bytes/sec, so each log line can take 50-100ms". |
| `:2465` | `serial_println!` | `schedule` | SCHEDULER HELD - inside Scheduler::schedule(&mut self) (:2149), the dispatch decision itself | runtime gate: if debug_log, where debug_log = _count < 5 \|\| _count % 500 == 0 on x86_64 and literal false elsewhere (:2155-2157) | not gate-pinned | partly - SCHEDULE_COUNT (:2145) and context_switch_count() already exist | **H1** | THE CALL-OUT: :2341 contradicts this file's own Key Rule at :21-22, which names log_serial_println! by name as forbidden under the scheduler lock. The x86 carve-out at :2151-2154 is that COM2 is a different mutex from COM1 - true, and narrower than the rule as written. The file's own comment at :2143-2144 prices the line: "Serial output is ~960 bytes/sec, so each log line can take 50-100ms". |
| `:2478` | `serial_println!` | `schedule` | SCHEDULER HELD - inside Scheduler::schedule(&mut self) (:2149), the dispatch decision itself | runtime gate: if debug_log, where debug_log = _count < 5 \|\| _count % 500 == 0 on x86_64 and literal false elsewhere (:2155-2157) | not gate-pinned | partly - SCHEDULE_COUNT (:2145) and context_switch_count() already exist | **H1** | THE CALL-OUT: :2341 contradicts this file's own Key Rule at :21-22, which names log_serial_println! by name as forbidden under the scheduler lock. The x86 carve-out at :2151-2154 is that COM2 is a different mutex from COM1 - true, and narrower than the rule as written. The file's own comment at :2143-2144 prices the line: "Serial output is ~960 bytes/sec, so each log line can take 50-100ms". |
| `:2497` | `serial_println!` | `schedule` | SCHEDULER HELD - inside Scheduler::schedule(&mut self) (:2149), the dispatch decision itself | runtime gate: if debug_log, where debug_log = _count < 5 \|\| _count % 500 == 0 on x86_64 and literal false elsewhere (:2155-2157) | not gate-pinned | partly - SCHEDULE_COUNT (:2145) and context_switch_count() already exist | **H1** | THE CALL-OUT: :2341 contradicts this file's own Key Rule at :21-22, which names log_serial_println! by name as forbidden under the scheduler lock. The x86 carve-out at :2151-2154 is that COM2 is a different mutex from COM1 - true, and narrower than the rule as written. The file's own comment at :2143-2144 prices the line: "Serial output is ~960 bytes/sec, so each log line can take 50-100ms". |
| `:3408` | `serial_println!` | `unblock` | SCHEDULER HELD - Scheduler::unblock | cfg-gated on target_arch = x86_64 at :3407 | not gate-pinned | yes - the ENQUEUE_* / WAKE_SITE_* atomics read out by emit_wake_attribution_counters() | **H1** |  |
| `:3595` | `serial_println!` | `block_current_for_signal_with_context` | SCHEDULER HELD - Scheduler::block_current_for_signal_with_context | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:3610` | `serial_println!` | `block_current_for_signal_with_context` | SCHEDULER HELD - Scheduler::block_current_for_signal_with_context | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:3634` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3641` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3681` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3699` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3711` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3719` | `serial_println!` | `unblock_for_signal` | SCHEDULER HELD - Scheduler::unblock_for_signal | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_SIGNAL | **H1** |  |
| `:3745` | `serial_println!` | `block_current_for_child_exit` | SCHEDULER HELD - Scheduler::block_current_for_child_exit | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:3794` | `serial_println!` | `unblock_for_child_exit` | SCHEDULER HELD - Scheduler::unblock_for_child_exit | cfg-gated on target_arch = x86_64 | not gate-pinned | partly - WAKE_SITE_CHILD | **H1** |  |
| `:4965` | `crate::serial_println!` | `dump_thread_placement` | thread-context diagnostic - dump_thread_placement snapshots under one lock acquisition and prints AFTER release; the doc at :4885-4888 says so and says "intended for thread context" | ungated | not gate-pinned | not yet | **H3** | Correct by construction; the checker cannot see the release. |
| `:4973` | `crate::serial_println!` | `dump_thread_placement` | thread-context diagnostic - dump_thread_placement snapshots under one lock acquisition and prints AFTER release; the doc at :4885-4888 says so and says "intended for thread context" | ungated | not gate-pinned | not yet | **H3** | Correct by construction; the checker cannot see the release. |
| `:4984` | `crate::serial_println!` | `dump_thread_placement` | thread-context diagnostic - dump_thread_placement snapshots under one lock acquisition and prints AFTER release; the doc at :4885-4888 says so and says "intended for thread context" | ungated | not gate-pinned | not yet | **H3** | Correct by construction; the checker cannot see the release. |
| `:5190` | `serial_println!` | `init` | SCHEDULER HELD and IRQs masked - inside without_interrupts(\|\| { let mut scheduler_lock = lock_scheduler(); ... }) in init()/init_with_current() | cfg-gated on target_arch = x86_64 | xtask/src/boot_stages.rs, tests/arm64_boot_post_test.rs and docker/qemu/run-aarch64-kthread-parallel.sh pin "Scheduler initialized" | not yet | **H1** | The in-code comment says "Only log on x86_64 to avoid deadlock on ARM64" - an explicit acknowledgement that the shape IS the deadlock shape. The aarch64 side already emits the same marker correctly from main_aarch64.rs:864, outside the guard. |
| `:5221` | `serial_println!` | `init_with_current` | SCHEDULER HELD and IRQs masked - inside without_interrupts(\|\| { let mut scheduler_lock = lock_scheduler(); ... }) in init()/init_with_current() | cfg-gated on target_arch = x86_64 | xtask/src/boot_stages.rs, tests/arm64_boot_post_test.rs and docker/qemu/run-aarch64-kthread-parallel.sh pin "Scheduler initialized" | not yet | **H1** | The in-code comment says "Only log on x86_64 to avoid deadlock on ARM64" - an explicit acknowledgement that the shape IS the deadlock shape. The aarch64 side already emits the same marker correctly from main_aarch64.rs:864, outside the guard. |
| `:5757` | `crate::serial_println!` | `note_scheduler_publication` | PROCESS_MANAGER HELD BY CONSTRUCTION - the print arm is guarded by process_manager_held_on_current_cpu(), so it executes only when PM is held | ungated | docker/qemu/{run-aarch64-boot-test-strict,run-aarch64-boot-test-native,run-aarch64-full-test,run-x86-boot-tests,run-x86-prod-profile-boot-test}.sh; tests/exec_lock_order_structure.rs; tests/teardown_structure.rs (marker CREATION_LOCK_ORDER) | yes - CREATION_PUBLICATIONS_PM_HELD (:5744) | **H1** | Measured rate is 0 today, so the shape has fired 0 times; the counter is the real evidence and the line is the gate's handle. |
| `:5769` | `crate::serial_println!` | `probe_publication_lock_order_injection` | PROCESS_MANAGER HELD BY CONSTRUCTION - same predicate, deliberate injection probe | cfg-gated on feature = boot_tests at :5764 | same CREATION_LOCK_ORDER gates | yes - CREATION_PUBLICATIONS_PM_HELD_INJECTED | **H1** | This one is DESIGNED to fire, which is what makes the production 0 non-vacuous. |
| `:5893` | `crate::serial_println!` | `apply` | PROCESS_MANAGER HELD BY CONSTRUCTION - ExecSchedCommit::apply, pm_held arm, inside without_interrupts | ungated | docker/qemu/*boot-test*.sh; tests/exec_lock_order_structure.rs (marker EXEC_LOCK_ORDER) | yes - SCHED_AFTER_PM_VIOLATIONS | **H1** |  |
| `:5897` | `crate::serial_println!` | `apply` | ExecSchedCommit::apply, inside without_interrupts, scheduler guard already out of scope; the code comment at :5890-5891 states the lock is taken deliberately so a concurrent writer cannot tear the gate-pinned bytes | ungated | docker/qemu/*boot-test*.sh; tests/exec_lock_order_structure.rs | yes - EXEC_COMMIT_UNPINNED / EXEC_COMMIT_MISSING_THREAD / EXEC_SCHED_COMMITS | **H3** | Documented and deliberate; the fact is already an atomic and only the emission is a lock. |
| `:5901` | `crate::serial_println!` | `apply` | ExecSchedCommit::apply, inside without_interrupts, scheduler guard already out of scope; the code comment at :5890-5891 states the lock is taken deliberately so a concurrent writer cannot tear the gate-pinned bytes | ungated | docker/qemu/*boot-test*.sh; tests/exec_lock_order_structure.rs | yes - EXEC_COMMIT_UNPINNED / EXEC_COMMIT_MISSING_THREAD / EXEC_SCHED_COMMITS | **H3** | Documented and deliberate; the fact is already an atomic and only the emission is a lock. |
| `:5904` | `crate::serial_println!` | `apply` | ExecSchedCommit::apply, inside without_interrupts, scheduler guard already out of scope; the code comment at :5890-5891 states the lock is taken deliberately so a concurrent writer cannot tear the gate-pinned bytes | ungated | docker/qemu/*boot-test*.sh; tests/exec_lock_order_structure.rs | yes - EXEC_COMMIT_UNPINNED / EXEC_COMMIT_MISSING_THREAD / EXEC_SCHED_COMMITS | **H3** | Documented and deliberate; the fact is already an atomic and only the emission is a lock. |
| `:6278` | `log::info!` | `switch_to_idle` | SCHEDULER HELD - inside with_scheduler(\|sched\| ...) in switch_to_idle(), which x86 exception handlers call | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:6284` | `log::error!` | `switch_to_idle` | SCHEDULER HELD - inside with_scheduler(\|sched\| ...) in switch_to_idle(), which x86 exception handlers call | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:6291` | `log::info!` | `switch_to_idle` | SCHEDULER HELD - inside with_scheduler(\|sched\| ...) in switch_to_idle(), which x86 exception handlers call | cfg-gated on target_arch = x86_64 | not gate-pinned | not yet | **H1** |  |
| `:6354` | `log::error!` | `abort_dispatch_and_resume` | SCHEDULER HELD - inside with_scheduler(\|sched\| ...) in abort_dispatch_and_resume(), called from the IRQ dispatch path (interrupts/context_switch.rs:466, :838, :1199) | cfg-gated on target_arch = x86_64, on the fn (:6346) | not gate-pinned | not yet | **H1** |  |
| `:6502` | `log::info!` | `test_unblock_does_not_duplicate_ready_queue` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6526` | `log::info!` | `test_unblock_does_not_duplicate_ready_queue` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6530` | `log::info!` | `test_schedule_does_not_duplicate_ready_queue` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6559` | `log::info!` | `test_schedule_does_not_duplicate_ready_queue` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6572` | `log::info!` | `test_yield_current_does_not_modify_scheduler_state` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6576` | `log::info!` | `test_yield_current_does_not_modify_scheduler_state` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6583` | `log::info!` | `test_yield_current_does_not_modify_scheduler_state` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6605` | `log::info!` | `test_yield_current_does_not_modify_scheduler_state` | not compiled into any kernel - inside `pub mod tests`, cfg-gated on test AND target_arch = x86_64 (:6476) | cfg-gated on test AND target_arch = x86_64, on the module | not gate-pinned | n/a | **H4** | FALSE POSITIVE. The script should skip items under a test-gated cfg attribute; 0 of the kernel build profiles set cfg(test). |
| `:6623` | `log::info!` | `run_scheduler_tests` | dead code - run_scheduler_tests carries #[allow(dead_code)] and has 0 callers in kernel/src | cfg-gated on target_arch = x86_64, with a not(test) inner block | not gate-pinned | n/a | **H3** | CLAUDE.md forbids #[allow(dead_code)] on code that should be removed. Straight delete. |
| `:6630` | `log::error!` | `run_scheduler_tests` | dead code - run_scheduler_tests carries #[allow(dead_code)] and has 0 callers in kernel/src | cfg-gated on target_arch = x86_64, with a not(test) inner block | not gate-pinned | n/a | **H3** | CLAUDE.md forbids #[allow(dead_code)] on code that should be removed. Straight delete. |
| `:6636` | `log::info!` | `run_scheduler_tests` | dead code - run_scheduler_tests carries #[allow(dead_code)] and has 0 callers in kernel/src | cfg-gated on target_arch = x86_64, with a not(test) inner block | not gate-pinned | n/a | **H3** | CLAUDE.md forbids #[allow(dead_code)] on code that should be removed. Straight delete. |

### `kernel/src/syscall/handler.rs` — 2 lines (H2 2) — **TIER 1**

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:502` | `log::warn!` | `rust_syscall_handler` | TIER 1. Cold arm of rust_syscall_handler (#[no_mangle], :189), inside the preempt_disable() bracket; the file header at :185-186 says "NO logging, NO serial output" | ungated | not gate-pinned | not yet | **H2** |  |
| `:536` | `log::error!` | `rust_syscall_handler` | TIER 1. Syscall-return arm of rust_syscall_handler, inside the preempt_disable() bracket | ungated | not gate-pinned | not yet | **H2** | Unbounded: it fires on each syscall return for as long as kernel_stack_top == 0. |

### `kernel/src/syscall/time.rs` — 1 lines (H3 1) — **TIER 1**

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:114` | `log::info!` | `sys_clock_settime` | TIER 1. sys_clock_settime; the function doc at :92 states "This is NOT a hot path (called once per NTP sync), so logging is acceptable." | ungated | not gate-pinned | not yet | **H3** | The one site in the tree carrying an explicit in-code acceptance argument - and the script has no way to record it. |

### `kernel/src/arch_impl/aarch64/exception.rs` — 9 lines (H2 1 · H3 8)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:1450` | `crate::serial_println!` | `handle_sync_exception` | exception-handler body - the BRK_AARCH64 arm of handle_sync_exception (#[no_mangle], :666); blocking SERIAL1 from exception context | ungated | not gate-pinned | not yet | **H2** | Cold (only a BRK instruction reaches it), but it is a handler body. |
| `:1889` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback: handle_syscall is reached only from the "From EL1 (kernel) - shouldn't happen normally" arm at :702; EL0 syscalls go to rust_syscall_handler_aarch64 | ungated | not gate-pinned | not yet | **H3** |  |
| `:1890` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1891` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1892` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1893` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1894` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1895` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback (handle_syscall exit arm) | ungated | docker/qemu/run-aarch64-test-suite.sh:182 greps "Userspace Test Complete", but the LIVE producer is syscall_entry.rs:399 on the EL0 path | not yet | **H3** | Duplicate of the live banner; this copy is reachable only from the EL1 arm. |
| `:1947` | `crate::serial_println!` | `handle_syscall` | EL1-only fallback: handle_syscall is reached only from the "From EL1 (kernel) - shouldn't happen normally" arm at :702; EL0 syscalls go to rust_syscall_handler_aarch64 | ungated | not gate-pinned | not yet | **H3** |  |

### `kernel/src/arch_impl/aarch64/context_switch.rs` — 6 lines (H3 6)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:573` | `crate::serial_println!` | `record_resume_pc_refusal_locked` | thread-context one-shot oracle print (reached via run_deferred_reclamation, documented "before entering any interrupt-masked scheduling window", :6544) | ungated (the lock-free sibling record_resume_pc_refusal at :527 is the raw_uart_* variant) | docker/qemu/run-aarch64-service-sequence-gate.sh; tests/strand_handoff_structure.rs (marker RESUME_PC_REFUSED) | RESUME_PC_*_REFUSALS already counted; emission bounded to 16 by RESUME_PC_REFUSAL_EMISSIONS | **H3** | Deliberate locked/raw split. The safety argument is a CALLER contract that 0 in-tree checks pin. |
| `:791` | `crate::serial_println!` | `drain_asm_resume_pc_refusals` | thread-context one-shot oracle print (drain_asm_resume_pc_refusals, same caller) | ungated | not gate-pinned | RESUME_PC_CUSTODY_CHECKS / RESUME_PC_FOREIGN_REPORTS already counted | **H3** |  |
| `:842` | `crate::serial_println!` | `report_foreign_resume_pc_refusal` | thread-context one-shot oracle print (report_foreign_resume_pc_refusal) | ungated | not gate-pinned | RESUME_PC_FOREIGN_REPORTS; bounded to 16 by RESUME_PC_CUSTODY_EMISSIONS | **H3** |  |
| `:915` | `crate::serial_println!` | `emit_resume_pc_census_locked` | thread-context census read-out (emit_resume_pc_census_locked via emit_resume_pc_census_if_due) | ungated (the lock-free sibling emit_resume_pc_census at :873) | not gate-pinned | yes - the line prints only the RESUME_PC_CENSUS[][] / percpu_stack_selection_routed() atomics | **H3** | Pure atomic read-out: the fact is already published; only the emission is a lock. |
| `:930` | `crate::serial_println!` | `emit_resume_pc_census_locked` | thread-context census read-out (emit_resume_pc_census_locked via emit_resume_pc_census_if_due) | ungated (the lock-free sibling emit_resume_pc_census at :873) | not gate-pinned | yes - the line prints only the RESUME_PC_CENSUS[][] / percpu_stack_selection_routed() atomics | **H3** | Pure atomic read-out: the fact is already published; only the emission is a lock. |
| `:3273` | `crate::serial_println!` | `report_user_rsp_scratch_el_census` | boot_tests-only census read-out (report_user_rsp_scratch_el_census) | cfg-gated on feature = boot_tests, on the fn (:3267) | not gate-pinned | yes - USER_RSP_SCRATCH_EL0_INSTALLS / _EL1_SKIPPED | **H3** |  |

### `kernel/src/arch_impl/aarch64/timer_interrupt.rs` — 18 lines (H3 18)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:237` | `log::info!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:251` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:282` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:294` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:310` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:335` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:342` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:375` | `crate::serial_println!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:387` | `log::info!` | `init` | init-time only - timer_interrupt::init(), called once from main_aarch64.rs:949 | ungated | tests/arm64_boot_post_test.rs pins "[timer] Timer configured for" (:251) | not yet | **H3** | The IRQ handler itself in this file is clean. |
| `:810` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:844` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:852` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:872` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:885` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:913` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:916` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:930` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |
| `:937` | `crate::serial_println!` | `dump_gic_state` | init-time only - dump_gic_state(), single caller timer_interrupt.rs:303 inside init() | ungated | not gate-pinned | not yet | **H3** |  |

### `kernel/src/per_cpu.rs` — 12 lines (H2 1 · H3 11)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:375` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:388` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:389` | `log::debug!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:390` | `log::debug!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:425` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:429` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:439` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:447` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:452` | `log::info!` | `init` | init-time only - per_cpu::init() | ungated | xtask/src/boot_stages.rs pins HAL_PERCPU_INITIALIZED (:452) and "Per-CPU data initialized" (:388) | not yet | **H3** | Two of these nine are boot-stage markers: deleting them breaks the x86 boot-stage runner. |
| `:1160` | `log::warn!` | `set_kernel_cr3` | init-time only - set_kernel_cr3() | ungated | not gate-pinned | not yet | **H3** |  |
| `:1165` | `log::info!` | `set_kernel_cr3` | init-time only - set_kernel_cr3() | ungated | not gate-pinned | not yet | **H3** |  |
| `:1378` | `log::warn!` | `can_schedule` | IRQ-return path - can_schedule() is the first call in check_need_resched_and_switch (:161), so this runs on each timer IRQ return; the print arm fires only while current_thread is unset | runtime gate: bounded to the first 10 by EARLY_RETURN_COUNT | not gate-pinned | not yet | **H2** | This function already contains a lock-free raw_serial primitive (anchored in tests/serial_line_atomicity_structure.rs), so the replacement is in reach. |

### `kernel/src/per_cpu_aarch64.rs` — 6 lines (H3 6)

| Line | Call | Enclosing fn | Context | Feature/runtime gating | Pinned by | Already an atomic? | Hazard | Note |
|---|---|---|---|---|---|---|---|---|
| `:382` | `log::info!` | `init` | init-time only - per_cpu_aarch64::init() | ungated | xtask/src/boot_stages.rs and tests/arm64_boot_post_test.rs pin "Per-CPU data initialized" (:386) | not yet | **H3** |  |
| `:386` | `log::info!` | `init` | init-time only - per_cpu_aarch64::init() | ungated | xtask/src/boot_stages.rs and tests/arm64_boot_post_test.rs pin "Per-CPU data initialized" (:386) | not yet | **H3** |  |
| `:390` | `log::debug!` | `init` | init-time only - per_cpu_aarch64::init() | ungated | xtask/src/boot_stages.rs and tests/arm64_boot_post_test.rs pin "Per-CPU data initialized" (:386) | not yet | **H3** |  |
| `:400` | `log::info!` | `init` | init-time only - per_cpu_aarch64::init() | ungated | xtask/src/boot_stages.rs and tests/arm64_boot_post_test.rs pin "Per-CPU data initialized" (:386) | not yet | **H3** |  |
| `:414` | `log::info!` | `init` | init-time only - per_cpu_aarch64::init() | ungated | xtask/src/boot_stages.rs and tests/arm64_boot_post_test.rs pin "Per-CPU data initialized" (:386) | not yet | **H3** |  |
| `:842` | `log::warn!` | `set_kernel_cr3` | init-time only - set_kernel_cr3() | ungated | not gate-pinned | not yet | **H3** |  |
---

## 5. The two Tier-1 files, specifically

`kernel/src/syscall/handler.rs` and `kernel/src/syscall/time.rs` are on CLAUDE.md's Tier-1 list
("edit only to repair a defect that lives here"). Three lines total.

| Line | What it is | Does the defect live here? | Minimal + alone? | Absolute constraints |
|---|---|---|---|---|
| `handler.rs:502` | `log::warn!("Unknown syscall number: {} - returning ENOSYS", …)` on the unmatched-syscall arm of the dispatch match, inside the `preempt_disable()` bracket | Yes. The prohibited call is *in* this file's own hot-path function; 0 changes outside this file can remove it. | Yes: delete the line, add one `AtomicU64` increment. Commit touches this file only. | The change **removes** a lock, a format and an I/O; it adds a relaxed `fetch_add`. 0 items from the refused list are added. |
| `handler.rs:536` | `log::error!("CRITICAL: Cannot set TSS.RSP0 - kernel_stack_top is 0!")` on the syscall-return path | Yes, same reasoning. And this one is **unbounded**: it fires on each syscall return for as long as the condition holds, so the file's own "NO logging, NO serial output" contract at :185-186 is violated on a repeating path, not a one-shot one. | Yes. | Same. |
| `time.rs:114` | `log::info!("clock_settime: wall clock adjusted to …")` in `sys_clock_settime` | Arguable. The function doc at :92 states the acceptance argument in code: "This is NOT a hot path (called once per NTP sync), so logging is acceptable." | Either delete (one line) or leave it and give the checker an annotation that can record the argument. | Unchanged either way. |

Each of the three ships as its own single-file commit, with the PR body carrying the four
statements CLAUDE.md asks for (which file, why the defect lives there, why no non-Tier-1 change
repairs it, what it costs on the path) and with **boot evidence, not a build** — Tier-1 condition 5.

## 6. The specific call-out: `scheduler.rs:2341`

```rust
// scheduler.rs:2149   pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
// scheduler.rs:2155   #[cfg(target_arch = "x86_64")]
// scheduler.rs:2156   let debug_log = _count < 5 || (_count % 500 == 0);
// scheduler.rs:2157   #[cfg(not(target_arch = "x86_64"))]
// scheduler.rs:2158   let debug_log = false;
...
// scheduler.rs:2340   if debug_log {
// scheduler.rs:2341       log_serial_println!(
// scheduler.rs:2342           "Next thread from queue: {}, cpu: {}",
```

`schedule` is `&mut self` on `Scheduler`, so each of its callers reaches it through the SCHEDULER guard.
On x86_64 `log_serial_println!` resolves to `serial::_log_print` → `arch_disable_interrupts()` →
**blocking** `SERIAL2.lock()` → `.expect(…)`. That is a Level-4 acquisition under a Level-1 hold, in
the scheduling decision itself, which is exactly what this file's own Key Rule at `:21-22` names as
forbidden — and it names `log_serial_println!` by name, without an architecture qualifier.

The in-code defence at `:2151-2154` is that COM2 is a different UART from COM1, so the aarch64
single-PL011 deadlock cannot occur. That is correct as far as it goes, and it is **narrower than the
rule**: it answers the AB-BA question and leaves three costs standing.

1. The SCHEDULER hold is extended across UART output the same function prices at
   "~960 bytes/sec, so each log line can take 50-100ms" (`:2143-2144`).
2. `_log_print` masks interrupts for the whole acquisition, so those bytes are also an
   interrupts-off window inside the scheduler.
3. `.expect("Printing to log serial failed")` panics with `SERIAL2` held.

Five lines in `schedule()` are on this gate (`:2341`, `:2447`, `:2465`, `:2478`, `:2497`); the
`debug_log` predicate fires on the first 5 calls and then on each 500th, so this is periodic, not
one-shot.

---

## 7. Drain plan — R157 small PRs, ordered by hazard

Each PR is small, same-day, non-reversing, and carries a named mutation that must redden its own
ratchet, plus a main-health battery after merge. Column *After* is the count `scripts/check-critical-path-violations.sh` should report on the
next run, starting from 135.

### PR-0 — the census ratchet (lands first, changes no kernel code)

**What.** A new host-side structure suite, `tests/critical_path_logging_census_structure.rs`, that
reproduces the script's match set in Rust and pins it per `(file, item-path)` at today's numbers.

**Why in Rust rather than "make the script exit 0 with a baseline file".** The tree already has the
machinery and the house style: `tests/serial_line_atomicity_structure.rs` carries `code_mask`
(comment- and string-aware, which the shell grep is not), `item_spans`/`item_path_at`,
`census`/`expected_census`/`census_diff`/`validate_census`, and `with_synthetic_source` for
anti-vacuity. `tests/capture_path_lock_free_structure.rs` carries the pattern of keeping a Rust
denylist in step with this very script. PR-0 reuses both.

**What it pins.**

1. `CRITICAL_PATH_LOG_ANCHORS: &[(&str, &str, usize)]` — one row per `(file, item-path)`, summing
   to **135**. Census-anchored on item paths rather than line numbers, per the standing lesson (0 line pins).
2. The three checked files that are clean today — `arch_impl/aarch64/context.rs`,
   `interrupts/timer.rs`, `arch_impl/aarch64/percpu.rs` — pinned at **0**, so a first print there
   fails rather than quietly becoming a tenth `VIOLATION` header.
3. `CRITICAL_FILES` and `PROHIBITED_PATTERNS` as spelled in the shell script equal the Rust lists
   (the same discipline as `the_shell_guard_and_this_suite_deny_the_same_shapes`).
4. A **second, wider** census over `serial_print!`, `log_serial_print!` and `log::log!` — the
   spellings that reach the same locks and that the script's patterns miss. That census is **136**
   today: the 135 plus `arch_impl/aarch64/exception.rs :: fn sys_write` = 1. The shell script's
   pattern list is widened in the same PR so the two stay in step, which is why PR-0 is the PR that
   discloses the escaped site rather than leaving it for someone to find later.

**Direction.** Any increase fails. A decrease also fails, with a diff line naming the anchor
(`- <file> :: <item> (expected N, found 0)`), so a drain PR must update the table consciously —
that is the mechanism, not a nuisance.

**Anti-vacuity mutations** (each its own `#[test]`, each asserted to return `Err`):

| Mutation | Must redden |
|---|---|
| `with_synthetic_source` adds a file at a checked path containing one `serial_println!` in a new fn | a `+ <file> :: <item>` row for the synthetic site |
| One anchor row deleted from `CRITICAL_PATH_LOG_ANCHORS` | `+ …` for the now-unexpected real site |
| One anchor row's count decremented | `~ … (expected N-1, found N)` |
| Synthetic `interrupts/timer.rs` carrying one `log::info!` | the zero-pin fires |
| Shell `CRITICAL_FILES` entry removed | the in-step assertion fires |

**Wiring.** `cargo test --test critical_path_logging_census_structure`, invoked the way the other
structure suites are (`cargo test --test <name>`, host target, no `[[test]]` stanza needed), and
named in `docker/qemu/run-x86-boot-tests.sh` and
`docker/qemu/run-aarch64-boot-test-strict.sh` beside the existing structure-suite calls.

**Gates:** the two structure-suite invocations above plus `bash scripts/check-critical-path-violations.sh`
(record exit status and line count in the round notes) and `scripts/claim-lint.py`.
**After: 135** (unchanged — PR-0 moves no code).

---

### PR-1 — x86 dispatch path, lock-held prints (H1 ×16)

**Lines.** `kernel/src/interrupts/context_switch.rs` `:458 :649 :656 :662 :837 :980 :1030 :1079
:1092 :1111 :1194 :1232 :1337 :1429 :1499 :1512`.

**Mechanism.** Delete each print. Nine of the sixteen already have the fact published one or two
lines away as a `DispatchAbandonSite` counter (`:465` `RollbackSaveFailed`, `:839` `RollbackTls`,
`:1104` `IdleSignalTerminatedBlocked`, `:1200` `RollbackKernelContextLock`, `:1445`
`IdleRestoreError`), incremented by `trace_dispatch_abandon` — one relaxed per-site `fetch_add`, no
lock, no formatting. The seven that do not (`:649 :656 :662 :1092 :1337 :1499 :1512`) get one new
per-arm counter each in the same `DispatchAbandonSite`/`DispatchSaveReason` family. Emission moves
to the existing thread-context reporter `crate::task::report_dispatch_strand_census_heartbeat()`,
whose emission boundary already refuses to run with interrupts disabled
(`dispatch_strand_census.rs:156`) — that is the R188 shape, already built.

**Oracle / ratchet.** (a) A forced-leg oracle per new counter: drive the arm (the PM-unavailable
arm is reachable by holding `PROCESS_MANAGER` from a kthread while a timer IRQ lands) and show the
counter moves while no serial line appears. (b) Mutation: re-insert one deleted print in a scratch
build and show PR-0's census reddens with a `+` row. (c) The counter totals must equal the number of
forced legs, so a counter that silently does not increment fails.

**Gates.** `docker/qemu/run-x86-boot-tests.sh`, `docker/qemu/run-x86-prod-profile-boot-test.sh`,
`docker/qemu/run-boot-parallel.sh 5`, plus both structure suites. x86 runs on beast.
**Tier:** Tier 2 (`interrupts/context_switch.rs`, "Context switch path — timing sensitive"). The
change removes work from the path, which is the direction Tier 2 asks for; the PR body still states
why the edit belongs here.
**After: 119.**

---

### PR-2 — `Scheduler::schedule()` and the two add-thread seams (H1 ×7)

**Lines.** `kernel/src/task/scheduler.rs` `:1981 :2091 :2341 :2447 :2465 :2478 :2497`.

**Mechanism.** Delete. These are the direct contradiction of the file's own Key Rule at `:21-22`.
`SCHEDULE_COUNT` (`:2145`) and `context_switch_count()` (`:699`) already publish the volume; the
per-decision detail ("next thread from queue", "switching from X to Y") is already carried
losslessly by the `SCHED_PROVIDER` trace ring via `trace_sched_diag`/`trace_ctx_switch`, which is
lock-free by construction and dumpable from GDB (`call trace_dump_latest(20)`).

**Oracle / ratchet.** (a) Anti-vacuity: with the 7 prints gone, a GDB `trace_dump_latest` on a boot
must still show the dispatch sequence they described — if the trace ring does not carry it, the
deletion is not equivalent and the PR is wrong. (b) A re-insert mutation on the PR-0 census.
**Gates.** x86 boot tests + prod-profile + `run-boot-parallel.sh 5`; aarch64 unaffected
(`debug_log` is a literal `false` there) but `run-aarch64-boot-test-strict.sh` runs anyway to prove
no aarch64 movement. **Tier:** not tiered (`task/scheduler.rs` is on the checker's list but on neither of
CLAUDE.md's two tiers).
**After: 112.**

---

### PR-3 — scheduler block/unblock seams (H1 ×11)

**Lines.** `scheduler.rs` `:3408 :3595 :3610 :3634 :3641 :3681 :3699 :3711 :3719 :3745 :3794`.

**Mechanism.** Delete. 11 of 11 sit in `impl Scheduler` methods reached under the guard, and the
wake-attribution family (`WAKE_SITE_SIGNAL`, `WAKE_SITE_CHILD`, `ENQUEUE_*`) already counts most of
what they say; the two or three facts not yet counted (`unblock_for_signal: thread not found`,
`state != BlockedOnSignal`) get one counter each. Emission stays where it already is:
`emit_wake_attribution_counters()` (`:274`), called from `net/mod.rs:719` in kthread context.

**Oracle / ratchet.** Forced legs through the signal and `waitpid` paths with the counters read
before/after; census mutation.
**Gates.** x86 boot tests + prod-profile; `docker/qemu/run-aarch64-service-sequence-gate.sh` at the
25-boots-per-profile default. **After: 101.**

---

### PR-4 — lock-order detectors and the exception idle path (H1 ×9)

**Lines.** `scheduler.rs` `:5190 :5221` (init under the guard), `:5757 :5769 :5893` (the
lock-order detectors, which print *precisely when* PROCESS_MANAGER is held), `:6278 :6284 :6291`
(`switch_to_idle` under `with_scheduler`), `:6354` (`abort_dispatch_and_resume` under
`with_scheduler`, called from the IRQ dispatch path).

**Mechanism, split three ways.**
- `:5190 :5221` — move the emission out of the guard. The aarch64 side already does exactly this:
  `main_aarch64.rs:864` prints `[boot] Scheduler initialized` after `init_with_current` returns. Do
  the same on x86 from `main.rs`, preserving the exact marker text `xtask/src/boot_stages.rs` and
  `tests/arm64_boot_post_test.rs` look for.
- `:5757 :5769 :5893` — the counters `CREATION_PUBLICATIONS_PM_HELD`,
  `CREATION_PUBLICATIONS_PM_HELD_INJECTED` and `SCHED_AFTER_PM_VIOLATIONS` already are the evidence;
  the prints are the *gates'* handle on them. Move the emission to a thread-context reporter that
  prints the same `[CREATION_LOCK_ORDER:…]` / `[EXEC_LOCK_ORDER:…]` lines from the counters, and
  update `docker/qemu/{run-aarch64-boot-test-strict,run-aarch64-boot-test-native,run-aarch64-full-test,run-x86-boot-tests,run-x86-prod-profile-boot-test}.sh`,
  `tests/exec_lock_order_structure.rs` and `tests/teardown_structure.rs` in the same PR. **This is
  the one PR in the plan that touches gate scripts**, so it carries the most review weight.
- `:6278 :6284 :6291 :6354` — delete; add counters.

**Anti-vacuity, load-bearing here.** `probe_publication_lock_order_injection` (`:5769`) exists to
make the production 0 non-vacuous. The moved reporter must keep that property: the boot_tests
injection leg must still produce `[CREATION_LOCK_ORDER:INJECTED:PM_HELD]` on the gate's stdout, and
the round must show it red-when-injected and green-when-not, on the same build.
**Gates.** The 5 gates listed above, both architectures, plus `./run.sh --parallels` (this
touches a kernel merge path). **After: 92.**

---

### PR-5 — x86 IRQ-context prints with no lock held (H2 ×6)

**Lines.** `interrupts/context_switch.rs` `:1369 :1582 :1683 :1712 :1714`; `per_cpu.rs :1378`.

**Mechanism.** `:1582` gets the existing `note_dispatch_guard_unavailable()` (`:121`), which today
is called only from `:353`, so the PM-unavailable fact is already half-published. `:1369` and
`:1683 :1712 :1714` become `raw_serial_str` breadcrumbs beside the ones already in this function
(`:407` `RING3_ENTER`, `:445` `<S>`, `:812` `[SW]`) or plain counters. `per_cpu.rs:1378` becomes a
`raw_serial_str` — `can_schedule` **already contains a lock-free raw serial primitive**, anchored as
`("kernel/src/per_cpu.rs", "fn can_schedule", 1)` in
`tests/serial_line_atomicity_structure.rs:759`, so the replacement is in-function and the anchor
count moves 1 → 2 consciously.

**Note found en route, fix it here.** `tests/ring3_smoke_test.rs:26` looks for
`"RING3_ENTRY: Thread entering Ring 3"`. No file under `kernel/` produces that string; the nearest
producers are `interrupts/context_switch.rs:1684` (`"RING3_ENTRY: RIP=…"`) and `:407`
(`"RING3_ENTER: CS=0x33"`). That clause is dead. Repair it in this PR rather than deleting `:1683`
and leaving a test whose condition was already unsatisfiable.

**Gates.** x86 boot tests, prod-profile, `run-boot-parallel.sh 5`, `tests/ring3_smoke_test.rs`.
**After: 86.**

---

### PR-6 — Tier 1: `syscall/handler.rs` (H2 ×2)

**Lines.** `:502`, `:536`. **Two commits, one file each is not possible here (same file), so: two
commits, one line each**, so each diff reads on its own — Tier-1 condition 2.

**Mechanism.** `:502` → `SYSCALL_ENOSYS_TOTAL.fetch_add(1, Relaxed)`; `:536` →
`TSS_RSP0_ZERO_TOTAL.fetch_add(1, Relaxed)`. Both published by the existing syscall trace provider's
counter dump, not by a print. The change *removes* a lock, a format and an I/O from a Tier-1 file
and adds one relaxed atomic — Tier-1 condition 4 is satisfied by subtraction.

**Oracle / ratchet.** `:502`: a userspace test binary issuing syscall number 0xFFFF; the counter must
read 1 and the serial log must contain no `Unknown syscall number`. `:536`: a `boot_tests`-only
injection that zeroes `kernel_stack_top` for one return, showing the counter moves. Both legs
mutation-proven: revert one line and the oracle goes red.
**Gates.** x86 boot tests + prod-profile + `run-boot-parallel.sh 5`, **with boot evidence attached**
(Tier-1 condition 5 — a passing build is explicitly not acceptance).
**Tier: 1.** **After: 84.**

---

### PR-7 — Tier 1: `syscall/time.rs` (H3 ×1)

**Line.** `:114` in `sys_clock_settime`.

**Mechanism.** Delete, and count the adjustment (`WALL_CLOCK_ADJUSTMENTS`). The alternative — keep
it and teach the checker an annotation — is deliberately *not* taken first: an annotation mechanism
introduced on a Tier-1 line is an annotation mechanism whose first use is its weakest case. Build
the mechanism in PR-9 where it has 35 well-argued users, then decide whether this line wants it.
**Gates.** x86 boot tests + prod-profile; boot evidence. **Tier: 1.** **After: 83.**

---

### PR-8 — aarch64 exception handler and the dead EL1 banner (H2 ×1, H3 ×8, plus the escaped site)

**Lines.** `arch_impl/aarch64/exception.rs` `:1450` (BRK arm of the `#[no_mangle]` sync handler),
`:1889-:1895` and `:1947` (the `handle_syscall` EL1-only fallback), and `:1985` — the
`crate::serial_print!` per-byte write that the current script does not see and PR-0's wider census
does.

**Mechanism.** `:1450` → `raw_uart_str`/`raw_uart_hex`, the primitives this file's neighbours
already use. `:1891-:1895` → delete: it is a byte-duplicate of the live banner at
`syscall_entry.rs:399`, which is what actually satisfies
`docker/qemu/run-aarch64-test-suite.sh:182` on the EL0 path; this copy is reachable only from the
"From EL1 (kernel) — shouldn't happen normally" arm at `:702`. `:1889` and `:1947` → `raw_uart_*`.
`:1985` → route `sys_write` through `write_bytes_atomic` (one lock for the whole buffer) instead of
one blocking acquisition per byte — a strict improvement on a path that today takes `count` locks.

**Oracle / ratchet.** Falsify the claim that the EL1 arm is the only reader, by mutation: a
`boot_tests` leg that issues an SVC from EL1 and shows the `raw_uart` line; and a normal boot showing
the banner still arrives from `syscall_entry.rs`. For `:1985`, a byte-count oracle: N bytes written → 1 serial acquisition, not N.
**Gates.** `run-aarch64-boot-test-native.sh`, `run-aarch64-boot-test-strict.sh`,
`run-aarch64-prod-profile-boot-test.sh`, `run-aarch64-test-suite.sh`, `./run.sh --parallels`.
**After: 74** (and the wider census 136 → 74).

---

### PR-9 — teach the checker an accepted set, and use it for the 35 init-time lines (H3 ×35)

**Lines.** `arch_impl/aarch64/timer_interrupt.rs`, 18 of 18; `per_cpu.rs` `:375 :388 :389 :390 :425
:429 :439 :447 :452 :1160 :1165`; `per_cpu_aarch64.rs`, 6 of 6.

**Mechanism — this is the "teach the checker" PR.** These are not hazards: they run once, from
`init()`, before the scheduler exists. Deleting them would cost real boot observability and would
break `xtask/src/boot_stages.rs` (`HAL_PERCPU_INITIALIZED`, `Per-CPU data initialized`) and
`tests/arm64_boot_post_test.rs` (`[timer] Timer configured for`). So the script learns to accept
them, and the acceptance is pinned rather than granted:

- A trailing marker the script and the Rust census both recognise:
  `// critical-path:accepted(init-time) — <one-line reason>` on the line above the call.
- The script subtracts marked lines from its violation count but **prints them under an
  `ACCEPTED in <file>` header with their reasons**, so an accepted set of 35 is visible rather than silent.
- PR-0's census gains a second anchor table, `CRITICAL_PATH_ACCEPTED_ANCHORS`, pinned per
  `(file, item-path)` at 35. Adding an annotation anywhere new is a `+` diff and fails; removing one
  is a `-` diff and fails. Acceptance becomes a reviewed edit to a table, which is the whole point.

**Anti-vacuity.** Two mutations: (a) annotate a line in `Scheduler::schedule()` — the accepted
census reddens, because that anchor is not in the table; (b) delete one annotation — the violation
census reddens. Both must be shown, or the mechanism is a rubber stamp.
**Gates.** Both architectures' boot tests, `xtask boot-stages`, `tests/arm64_boot_post_test.rs`.
**After: 39 flagged, 35 accepted.**

---

### PR-10 — the thread-context oracle emitters (H3 ×19, plus 3 dead lines)

**Lines.** `arch_impl/aarch64/context_switch.rs`, 6 of 6; `scheduler.rs` `:275 :285 :490 :531 :540
:569 :578` (census read-outs and the pin-guard oracle), `:4965 :4973 :4984`
(`dump_thread_placement`), `:5897 :5901 :5904` (`ExecSchedCommit::apply`); and delete
`:6623 :6630 :6636` with the dead `run_scheduler_tests` that contains them.

**Mechanism.** `critical-path:accepted(thread-context)`, same mechanism as PR-9, with a stronger
reason each: 19 of 19 are called from `run_deferred_reclamation()` (documented at
`context_switch.rs:6544` as running *before* any interrupt-masked window), from a kthread, or from
the boot thread; several print only the contents of atomics they just read; two have
lock-free `raw_uart_*` siblings in the same file (`record_resume_pc_refusal` `:527`,
`emit_resume_pc_census` `:873`) that make the locked/raw split deliberate; `dump_thread_placement`
snapshots under one acquisition and prints after release, as its own doc at `:4885-4888` says; and
`apply()`'s comment at `:5890-5891` states the lock is taken *on purpose* so the gate-pinned bytes
cannot be torn.

**What is not claimed.** The safety of these 19 rests on a **caller contract**, and 0 in-tree checks
enforce it: no check today stops a future caller invoking
`emit_resume_pc_census_locked` from an interrupt-masked window. The accepted-set annotation records
the argument; it does not verify it. Closing that gap needs a caller-side census (which callers may
reach an accepted-thread-context emitter), and that is deliberately **out of scope for this plan** —
it is the natural PR-12.

`run_scheduler_tests` is a straight delete: `#[allow(dead_code)]` with 0 callers in
`kernel/src`, which CLAUDE.md's code-quality section forbids.
**Gates.** Both architectures' boot tests, `run-aarch64-service-sequence-gate.sh`,
`run-aarch64-prod-profile-boot-test.sh`, `tests/loopback_pump_structure.rs`,
`tests/strand_handoff_structure.rs`. **After: 17 flagged, 54 accepted.**

---

### PR-11 — the false positive and the suppressed-trace residue (H4 ×8, H3 ×9), and the flip to 0

**Lines.** `scheduler.rs :6502 :6526 :6530 :6559 :6572 :6576 :6583 :6605` (inside `pub mod tests`,
cfg-gated on test AND target_arch = x86_64, `:6476`) and
`interrupts/context_switch.rs :557 :642 :748 :821 :882 :1262 :1352 :1392 :1497`.

**Mechanism.** Two independent halves.
- *The false positive.* The kernel is not built with `cfg(test)` in any of its profiles, so those 8
  lines cannot execute in a shipped kernel. **The script should skip items carried under a test-gated cfg
  attribute.** In the shell that means tracking the nearest preceding
  `#[cfg(…test…)]` attribute and its brace extent; in the Rust census it falls out of `item_spans`,
  which already reads item headers and their cfgs (`header_cfg`,
  `serial_line_atomicity_structure.rs:321`). Do it in the Rust census, and give the shell script the
  cheap version.
- *The residue.* The 9 `log::trace!` calls emit 0 bytes today (`CombinedLogger` drops Trace before
  taking a lock) but still evaluate their format arguments on each dispatch, and two of them
  (`:748`, `:821`) sit **inside `scheduler::with_thread_mut` closures**, i.e. under the SCHEDULER
  lock. They are armed: one logger change turns them into Level-1-holds-Level-4 in IRQ context.
  Delete them.

**Then flip the ratchet.** With the count at 0, PR-0's suite changes from "pins 135" to "pins 0
violations plus the 54-row accepted table", and
`bash scripts/check-critical-path-violations.sh` becomes a **gate** — added to
`docker/qemu/run-x86-boot-tests.sh` and `docker/qemu/run-aarch64-boot-test-strict.sh` as a
pass/fail step rather than a thing round docs record as "exit 1, unchanged from main".

**Anti-vacuity for the flip.** The gate must be shown red on a one-line mutation (add a
`serial_println!` to `interrupts/timer.rs`) and green without it, on the same build.
**Gates.** Both architectures, full set, plus `./run.sh --parallels`.
**After: 0 flagged, 54 accepted, script exits 0 and becomes a gate.**

---

### Summary ledger

| PR | Title | Lines | Tier | After |
|---|---|---|---|---|
| 0 | Census ratchet + widen the pattern set | 0 moved (pins 135; wider census 136) | — | 135 |
| 1 | x86 dispatch path, lock-held prints | 16 | 2 | 119 |
| 2 | `Scheduler::schedule()` + add-thread seams | 7 | — | 112 |
| 3 | Scheduler block/unblock seams | 11 | — | 101 |
| 4 | Lock-order detectors + exception idle path | 9 | — | 92 |
| 5 | x86 IRQ-context prints, no lock held | 6 | 2 | 86 |
| 6 | Tier 1: `syscall/handler.rs` | 2 | **1** | 84 |
| 7 | Tier 1: `syscall/time.rs` | 1 | **1** | 83 |
| 8 | aarch64 exception handler + dead EL1 banner + escaped `serial_print!` | 9 (+1) | — | 74 |
| 9 | Accepted-set mechanism, applied to init-time | 35 accepted | — | 39 |
| 10 | Thread-context oracle emitters + dead code | 19 accepted, 3 deleted | — | 17 |
| 11 | `cfg(test)` false positive + suppressed-trace residue, flip to gate | 17 | 2 | **0** |

---

## 8. What this document does not claim

- **No deadlock is claimed to have been observed** at any of the 43 H1 sites. The claim is the
  ordering shape (`scheduler.rs:21-22`), the extended lock hold, and the interrupts-off window — not
  a reproduction. Whether any H1 site has ever contributed to a hang (`#630`'s x86 livelock is the
  nearest candidate) is unexamined here.
- **No runtime measurement was taken.** The "~960 bytes/sec, 50-100ms per line" figure is quoted
  from `scheduler.rs:2143-2144`; it was not re-measured for this census.
- **No gate was run.** The only command executed against the tree was
  `bash scripts/check-critical-path-violations.sh` (exit 1, 274 lines) plus read-only greps. The
  per-PR "After" counts are arithmetic on the census, not observed gate output.
- **Reachability was read, not executed.** "Init-time only", "EL1-only fallback", "0 callers"
  are conclusions from call-site greps over `kernel/src`, not from traced boots. A caller reached
  through a function pointer or from outside `kernel/src` would not appear in those greps.
- **The `capture/` directory is reported clean by the script, and that is not re-derived here.**
  Its 4 files pass both denylists today; `tests/capture_path_lock_free_structure.rs` is its ratchet.
- **The accepted-set mechanism in PR-9/PR-10 records an argument; it does not verify one.** The
  caller-side check that would verify "thread context" is named as out of scope (PR-12).
- **`arch_impl/aarch64/context.rs`, `interrupts/timer.rs` and `arch_impl/aarch64/percpu.rs` are
  clean of the denied spellings only.** That is not a statement that they take no locks.
- **The two `.asm` entries in `CRITICAL_FILES` are grepped but cannot match**, since 12 of the 12 patterns
  are Rust macro spellings. They are neither covered nor claimed to be.
