# PR-B stage 2 — ratchet deletion mutations

Five structural ratchets were added to `tests/context_restore_structure.rs`.
Each censuses a shape derived from the code — the function that emits the
refusal record, the files that name `swapper/0`, the file's pure SPSR-mode
predicate — never a literal list of names and never a line number.

Each was proven non-vacuous by deleting its subject by hand, running the test,
recording the verbatim failure, and restoring. Command in every case:

```
cargo test --test context_restore_structure <test name>
```

Unmutated, all 77 tests in the suite pass.

---

## R1 — `percpu_stack_top_writers_consult_the_ownership_check`

**Rule.** Every function body in `kernel/src/arch_impl/aarch64/percpu.rs` that
writes `PERCPU_KERNEL_STACK_TOP_OFFSET` or `PERCPU_USER_RSP_SCRATCH_OFFSET`
through a `percpu_write_*` helper also calls the ownership check. The check's
identifier is derived from the file: it is the function whose body carries the
`[PERCPU_STACK_ALIEN:` record. The writer census must be non-empty.

**Mutation.** Deleted the ownership-check call from
`PerCpuOps::set_kernel_stack_top`, leaving the write:

```rust
    unsafe fn set_kernel_stack_top(addr: u64) {
-        if !percpu_stack_install_permitted(addr, Location::caller()) {
-            return;
-        }
        percpu_write_u64(PERCPU_KERNEL_STACK_TOP_OFFSET, addr);
```

**Verbatim failure.**

```
running 1 test
test percpu_stack_top_writers_consult_the_ownership_check ... FAILED

failures:

---- percpu_stack_top_writers_consult_the_ownership_check stdout ----

thread 'percpu_stack_top_writers_consult_the_ownership_check' panicked at tests/context_restore_structure.rs:5851:5:
these kernel/src/arch_impl/aarch64/percpu.rs functions write a per-CPU stack-top field without calling the ownership check `percpu_stack_install_permitted`: ["set_kernel_stack_top"] (writers censused: {"set_kernel_stack_top", "set_user_rsp_scratch"})
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    percpu_stack_top_writers_consult_the_ownership_check

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.00s

error: test failed, to rerun pass `--test context_restore_structure`
```

The check identifier and both writers are derived, not listed.

---

## R2 — `swapper_files_do_not_assign_thread_id_zero`

**Rule.** No file below `kernel/src` containing the literal `swapper/0` assigns
the bare literal `0` to a field named `id`. The file set is derived from the
literal and must be non-empty.

**Mutation.** Restored the deleted overwrite in `kernel/src/main_aarch64.rs`:

```rust
     idle_task.state = ThreadState::Running;
+    idle_task.id = 0;
     idle_task.has_started = true;
```

**Verbatim failure.**

```
running 1 test
test swapper_files_do_not_assign_thread_id_zero ... FAILED

failures:

---- swapper_files_do_not_assign_thread_id_zero stdout ----

thread 'swapper_files_do_not_assign_thread_id_zero' panicked at tests/context_restore_structure.rs:5924:5:
0 is the no-thread sentinel and must never be assigned to a live thread's id, but these `swapper/0` files do it: ["kernel/src/main_aarch64.rs:1631"] (files censused: ["kernel/src/main_aarch64.rs", "kernel/src/task/percpu_stack_oracle.rs", "kernel/src/main.rs"])
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    swapper_files_do_not_assign_thread_id_zero

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test context_restore_structure`
```

Three files carry the literal today. `kernel/src/task/spawn.rs`, whose
`create_idle_thread` had the same overwrite, is NOT in the derived set — it does
not name `swapper/0`. That site was found by the STEP-3 sweep and deleted; this
ratchet does not cover it, which is stated here rather than papered over.

---

## R3 — `reclaim_drops_thread_control_blocks_inside_the_masked_region`

**Rule.** In the `reclaim_terminated_threads` that masks interrupts, the
explicit `drop(` of the reclaimed binding lies inside the `without_interrupts(`
argument span (paren matching on the masked source) and after the closing brace
of the innermost block that holds the scheduler lock guard.

**Mutation.** Moved the drop back outside the masked region — the shape before
this change:

```rust
-    without_interrupts(|| {
-        let reclaimed_threads = { ...lock guard scope... };
-        drop(reclaimed_threads);
-    });
+    let reclaimed_threads = without_interrupts(|| { ...lock guard scope... });
+    drop(reclaimed_threads);
```

**Verbatim failure.**

```
running 1 test
test reclaim_drops_thread_control_blocks_inside_the_masked_region ... FAILED

failures:

---- reclaim_drops_thread_control_blocks_inside_the_masked_region stdout ----

thread 'reclaim_drops_thread_control_blocks_inside_the_masked_region' panicked at tests/context_restore_structure.rs:5976:5:
the reclaimed control blocks are dropped OUTSIDE the masked region: a thread control block's heap free would run with interrupts enabled on a borrowed stack (drop at 824, masked region [584, 817))
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    reclaim_drops_thread_control_blocks_inside_the_masked_region

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test context_restore_structure`
```

Offsets are relative to the function body, not the file, so reflowing the file
does not move them.

---

## R4 — `user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level`

**Rule.** Every `set_user_rsp_scratch` call in
`kernel/src/arch_impl/aarch64/context_switch.rs` that installs the per-CPU
kernel stack top — directly in the argument, or through a binding whose
initialiser is exactly the accessor call — is preceded, within its enclosing
function body, by a call to the file's pure SPSR exception-level predicate. That
predicate is derived as the single function whose whole body is one expression
masking `spsr` with the mode field, where the mode-mask spellings are the
literal plus any `const` in the file bound to it. Census: 2 installs.

The idle-return paths (`setup_idle_return_locked`, `inline_schedule_trampoline`
and the fault-idle return) install the CPU's OWN idle stack into this slot on
purpose — their frames ERET to EL1 and `boot.S` reads this slot for an EL1
return — so the census rule deliberately does not sweep them in. An earlier,
looser version of this rule ("any binding mentioning a stack top") did, and it
was wrong; the failure it produced is what identified the distinction.

**Mutation R4a** — deleted the guard inside the helper:

```rust
 fn ensure_user_rsp_scratch_for_el0(frame: &Aarch64ExceptionFrame) {
-    if !frame_returns_to_el0(frame) {
-        note_user_rsp_scratch_el(false);
-        return;
-    }
     note_user_rsp_scratch_el(true);
     let kst = Aarch64PerCpu::kernel_stack_top();
```

```
running 1 test
test user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level ... FAILED

failures:

---- user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level stdout ----

thread 'user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level' panicked at tests/context_restore_structure.rs:6145:5:
these kernel-stack-top installs into the EL0 scratch slot are not preceded by the pending-exception-level predicate `frame_returns_to_el0`: ["kernel/src/arch_impl/aarch64/context_switch.rs:2784 in fn ensure_user_rsp_scratch_for_el0"] (2 installs censused)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.04s

error: test failed, to rerun pass `--test context_restore_structure`
```

**Mutation R4b** — deleted the guard at the userspace-dispatch site:

```rust
-        if frame_returns_to_el0(frame) {
-            note_user_rsp_scratch_el(true);
-            unsafe {
-                Aarch64PerCpu::set_user_rsp_scratch(Aarch64PerCpu::kernel_stack_top());
-            }
-        } else {
-            note_user_rsp_scratch_el(false);
-        }
+        unsafe {
+            Aarch64PerCpu::set_user_rsp_scratch(Aarch64PerCpu::kernel_stack_top());
+        }
```

```
running 1 test
test user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level ... FAILED

failures:

---- user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level stdout ----

thread 'user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level' panicked at tests/context_restore_structure.rs:6145:5:
these kernel-stack-top installs into the EL0 scratch slot are not preceded by the pending-exception-level predicate `frame_returns_to_el0`: ["kernel/src/arch_impl/aarch64/context_switch.rs:4515 in fn dispatch_thread_locked"] (2 installs censused)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    user_rsp_scratch_kernel_stack_installs_follow_the_pending_exception_level

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.04s

error: test failed, to rerun pass `--test context_restore_structure`
```

Both censused installs are individually load-bearing: deleting either guard
names exactly that site.

---

## R5 — `init_cpu_publishes_both_idle_stack_top_and_stack_ownership`

**Rule.** `per_cpu_aarch64::init_cpu`'s body calls both per-CPU stack
publishers. Both names are derived from the code they write: the idle-stack-top
publisher is the single function in `kernel/src/per_cpu_aarch64.rs` that
volatile-writes the `idle_stack_top` field; the ownership publisher is the
single function in `kernel/src/arch_impl/aarch64/constants.rs` that
volatile-writes `PERCPU_STACK_OWNER_MAGIC`.

**Mutation.** Deleted the ownership publication from `init_cpu`:

```rust
-    crate::arch_impl::aarch64::constants::publish_percpu_stack_owner(cpu_id);
 }
```

**Verbatim failure.**

```
running 1 test
test init_cpu_publishes_both_idle_stack_top_and_stack_ownership ... FAILED

failures:

---- init_cpu_publishes_both_idle_stack_top_and_stack_ownership stdout ----

thread 'init_cpu_publishes_both_idle_stack_top_and_stack_ownership' panicked at tests/context_restore_structure.rs:6195:9:
`init_cpu` in kernel/src/per_cpu_aarch64.rs does not call `publish_percpu_stack_owner`; a CPU would run with an unpublished per-CPU stack fact (derived publishers: idle-stack-top `set_idle_stack_top`, stack-owner `publish_percpu_stack_owner`)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    init_cpu_publishes_both_idle_stack_top_and_stack_ownership

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 76 filtered out; finished in 0.00s

error: test failed, to rerun pass `--test context_restore_structure`
```

Both publisher names appear in the message because both were derived; only the
missing call fails the assertion.
