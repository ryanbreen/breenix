# Turn 11 Fix Proposal For Turn 12

## Proposed Fix

Patch the scheduler dequeue validation, not xHCI.

Preferred Turn 12 patch:

1. Keep the xHCI IRQ-driven completion conversion unchanged.
2. In `kernel/src/task/scheduler.rs`, change `pop_dispatchable_from_cpu_queue_excluding()` so validation is non-destructive:
   - Drop entries only when the thread is missing or `Terminated`.
   - For `excluded_tid`, not-`Ready`, current-on-another-CPU, remote-idle, or deferred-requeue entries, rotate the ID to the back of the same queue and continue scanning only the queue length that existed at function entry.
   - Return `None` after scanning that fixed count if no entry is dispatchable.
3. Keep the "do not dispatch current/deferred threads" invariant, but stop using dequeue as a cleanup mechanism for transitional scheduler state.
4. Audit the same-thread path after this change:
   - If `pop_next_dispatchable_thread_excluding()` returns `None`, preserve the existing current-thread handling without losing queue membership.
   - Confirm the AArch64 userspace-alone path clears `previous_thread` and restores `Running` before returning `None`.

Fallback if the targeted patch does not pass the fresh-deploy gate:

- Revert the `cb73f6e3` dequeue helper replacement entirely back to the `045dcd04` behavior, while keeping the safer enqueue-side checks that already prevent current/deferred threads from being newly queued.
- Re-implement stale protection later with explicit ownership states instead of inferred dequeue-time state.

## Why Targeted Patch First

The Turn 8 intent was valid: avoid dispatching stale ready-queue entries that are still current or deferred on another CPU. The problem is not the predicate itself; it is that failed predicates currently destroy queue entries.

Rotating non-dispatchable entries keeps the safety property while avoiding loss of runnable work during AArch64 handoff windows.

## Risk

Files likely touched:

- `kernel/src/task/scheduler.rs`

Files not expected to be touched:

- Tier-1 syscall/interrupt hot paths
- `kernel/src/interrupts/context_switch.rs`
- `kernel/src/interrupts/timer.rs`
- `kernel/src/syscall/*`
- xHCI source unless a separate cleanup is requested later

Risk level: moderate. The change is central scheduler logic, but it is outside the prohibited Tier-1 files and can be validated directly with the Parallels fresh-deploy CPU0 guard plus the original Turn 8 anti-regression scenario.

Do not add logging to interrupt or syscall paths. If more diagnosis is needed, use GDB or existing watchdog/counter output.

## Acceptance Criteria

Turn 12 should pass these gates:

1. Build warning/error grep is empty.
2. A fresh Parallels boot with `./run.sh --parallels --test 45` reaches the normal userland service set:
   - `[init] Boot script completed`
   - `bsshd started`
   - `bounce started`
3. CPU0 progresses well beyond the regression guard threshold:
   - at least `cpu0 ticks=30000`
   - no `CPU0 REGRESSION ALARM`
4. Re-run the original Turn 8 scheduler validation sample after the fresh-deploy pass:
   - no old DATA_ABORT / INSTRUCTION_ABORT regression
   - no CPU0=5 regression
5. Preserve the one-source-of-truth split from Turn 11:
   - xHCI remains on the Turn 5 IRQ-driven path
   - scheduler dequeue validation is the only functional fix surface

## Suggested Commit Shape

Use a source commit only in Turn 12, after the targeted scheduler patch passes:

```text
fix(arm64): make scheduler dequeue validation non-destructive

The Turn 8 dequeue filter correctly avoided dispatching current/deferred
threads, but it dropped non-dispatchable queue entries. On AArch64 those
states can be legitimate context-switch handoff windows, so dropping them
can strand early userland and trip the CPU0 fresh-deploy guard.
```
