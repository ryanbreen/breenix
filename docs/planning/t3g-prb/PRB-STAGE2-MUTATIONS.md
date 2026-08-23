# PR-B stage 2 — designated behaviour mutations

Four mutations, run one at a time against the live gate. Command in every case:

```
./docker/qemu/run-aarch64-percpu-stack-custody-gate.sh
```

Each rebuilds `--features boot_tests,percpu_stack_custody_oracle` for
`aarch64-breenix-kernel.json` and boots it under
`qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4`.

Unmutated, the gate PASSES:

```
ARM64 PERCPU STACK CUSTODY GATE: PASSED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=1:arm_verified=1:stimuli=1:accepted=0:overwritten=0:pad_intact=1:elr_slot=0xa11e00000000001f:spsr_slot=0xa11e000000000020:overlay=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=2:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=19777:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
[PERCPU_STACK_ALIEN:cpu=1:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:456]
[BOOT_TESTS:PASS]
[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1]
```

`git status` is clean and the gate is green again after the last revert.

---

## M1 — delete the owner/slot comparison in the setters' ownership check

```rust
 fn percpu_stack_install_permitted(addr: u64, site: &'static Location<'static>) -> bool {
     ...
-    if slot == cpu && published.map_or(true, |owner| owner == cpu) {
+    if published.map_or(true, |owner| owner == cpu) {
         return true;
     }
```

Everything else — the record, the counter, the emission budget, the publication
— untouched.

**Expected:** probe A back to `accepted=1` and no `[PERCPU_STACK_ALIEN:` record.
**Observed:** exactly that.

```
ARM64 PERCPU STACK CUSTODY GATE: FAILED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=3:arm_verified=1:stimuli=1:accepted=1:overwritten=33:pad_intact=1:elr_slot=0xffff0000404e6420:spsr_slot=0x5:overlay=1:FAIL]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=1:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=18679:foreign_occupancy=2:max_concurrent=1:worst_slot=7:worst_cpu=3:FAIL]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
```

`grep -c '[PERCPU_STACK_ALIEN:'` over the serial: **0**.

The save frame's own fingerprint is back — `overwritten=33` with
`pad_intact=1`, `spsr_slot=0x5` (EL1h), `elr_slot` a kernel text address, and
`overlay=1`.

---

## M2 — keep the comparison, delete `publish_percpu_stack_owner` from `init_cpu`

```rust
 pub fn init_cpu(cpu_id: usize) {
     ...
     set_idle_stack_top(cpu_id, ...);
-    crate::arch_impl::aarch64::constants::publish_percpu_stack_owner(cpu_id);
 }
```

**Expected:** the refusal still happens; the `[PERCPU_STACK_ALIEN:` requirement
is still satisfied; the probes still pass; the record's owner field reads
`unpublished`.

**Observed:** all of that, and the gate stays GREEN.

```
ARM64 PERCPU STACK CUSTODY GATE: PASSED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=3:arm_verified=1:stimuli=1:accepted=0:overwritten=0:pad_intact=1:elr_slot=0xa11e00000000001f:spsr_slot=0xa11e000000000020:overlay=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=3:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=20179:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
```

Record lines, verbatim:

```
[PERCPU_STACK_ALIEN:cpu=3:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:455]
[PERCPU_STACK_ALIEN:cpu=3:owner=unpublished:sp=0xffff000044000000:tid=1202:site=kernel/src/task/percpu_stack_oracle.rs:456]
```

This is the result M2 was designed to produce: the refusal is keyed on the
ARITHMETIC (`slot != cpu`), not on publication, so removing publication does not
open a hole. Nothing passes silently that should not.

**One thing M2 does NOT demonstrate, stated rather than glossed:** the record's
owner field does not visibly degrade, because it already reads `unpublished` in
the unmutated build. Probe A deliberately borrows an OFFLINE CPU's slot (CPU 7
under `-smp 4`), and an offline CPU never ran `init_cpu`, so its sentinel is
unpublished either way. No refusal anywhere in this boot names an online CPU's
slot, so no record in either build carries a numeric owner. What M2 does prove
is the more important half: with publication gone, the own-slot accept falls
through to the `published == None` arm (probe B still accepts both its cases,
probe C still censuses 0 foreign installs) while the foreign refusal is
unaffected.

---

## M3 — delete probe C's `slot != cpu` comparison, stacked on M1

Run with M1 still applied, so probe A is red. Removing the comparison removes
what it guards:

```rust
 pub fn note_stack_top_install(value: u64) {
     ...
-    if slot != cpu {
-        C_WORST_SLOT.store(slot as u64, Ordering::Release);
-        C_WORST_CPU.store(cpu as u64, Ordering::Release);
-        C_FOREIGN_OCCUPANCY.fetch_add(1, Ordering::Release);
-    }
 }
```

**Expected:** probe C reports `foreign_occupancy=0` and PASSes while probe A is
red — proving the comparison is what produces C's verdict.
**Observed:** exactly that.

```
ARM64 PERCPU STACK CUSTODY GATE: FAILED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=1:arm_verified=1:stimuli=1:accepted=1:overwritten=33:pad_intact=1:elr_slot=0xffff00004051c498:spsr_slot=0x5:overlay=1:FAIL]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=2:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=19363:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=1:zero_resolves=0:PASS]
```

`observations=19363` is the anti-vacuity half: the census still watched the
dispatch path 19 363 times and still reported zero, because the only thing that
could have made it nonzero was deleted. Both M1 and M3 reverted afterwards.

---

## M4 — restore `idle_task.id = 0;` in `main_aarch64.rs`

```rust
     idle_task.state = ThreadState::Running;
+    idle_task.id = 0;
     idle_task.has_started = true;
```

**Expected:** probe D red.
**Observed:** red, on `swapper_tid`.

```
ARM64 PERCPU STACK CUSTODY GATE: FAILED
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=A:target_cpu=7:stimulus_cpu=0:arm_verified=1:stimuli=1:accepted=0:overwritten=0:pad_intact=1:elr_slot=0xa11e00000000001f:spsr_slot=0xa11e000000000020:overlay=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=B:cpu=1:own_top_accepted=1:heap_stack_accepted=1:target_image_disturbed=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=C:slots=8:observations=18280:foreign_occupancy=0:max_concurrent=1:worst_slot=0:worst_cpu=0:PASS]
[PERCPU_STACK_CUSTODY_ORACLE:aarch64:leg=D:swapper_tid=0:zero_resolves=0:FAIL]
```

`zero_resolves` stays 0 under M4, because `registered_idle_cpu` keeps its `!= 0`
empty-slot sentinel test — with CPU 0's idle thread back at id 0, its slot reads
as unregistered rather than resolving. Probe D checks both facts and this
mutation targets the first, so the first is the one that fails. Reverted
afterwards.
