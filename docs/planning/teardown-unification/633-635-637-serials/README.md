# Preserved serials — the aarch64 resume-PC consumer family (#635, #633, #637, #576, #626)

Every serial here was recorded by booting a kernel built from a named commit on
`fix/producer-corruption-family` under

```
qemu-system-aarch64 -M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4 \
  -kernel <kernel> -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 \
  -drive if=none,id=ext2,format=raw,file=<copy of target/ext2-aarch64.img>,throttling.iops-total=2000 \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 -serial file:<serial>
```

for 60 s, always with the soft-float kernel target `aarch64-breenix-kernel.json`.

Three commits are referenced throughout:

* **`10dbae3c`** — the census and the first five self-tests exist; no production guard has changed.
* **`f16c87a2`** — the self-test repairs (exactly-once drain, whole-line emission, transient EL0
  stimulus, arm-specific verdicts, the frame-side EL0 twin). Still no production guard change.
* **`704eb771`** — the production change: one window predicate for every EL1 consumer, and the EL0
  arm in both languages.

Read "before" as `10dbae3c` for leg P and leg Z and as `f16c87a2` for the four EL0 legs, which did
not exist in a usable form until the repairs landed.

---

## What each leg injects, and which arm it is aiming at

| leg | feature(s) | writes | arm under test |
|---|---|---|---|
| P | `resume_pc_el1_oracle` | its own exception frame's address into `frame.elr` of an EL1 frame | the assembly EL1 window at whichever epilogue consumes the frame |
| Z | `eret_zero_pc_oracle` | `0` into the same slot | the same arm, at the value #576/#626 are filed at |
| UK | `resume_pc_el0_kernel_oracle` | a kernel high-half data address into a userspace thread's saved resume PC | the Rust EL0 arm at the two producers |
| UT | `resume_pc_el0_tid_oracle` | the thread's own tid into the same field | the same |
| FK | `+ resume_pc_el0_frame_oracle` | the kernel address straight into an EL0-targeted `frame.elr` | the assembly EL0 arm |
| FT | `+ resume_pc_el0_frame_oracle` | the tid straight into the same slot | the same |

`resume_pc_oracle_disarm` runs each harness to its injection point and skips only the store.

## Red before

| file | commit | what it shows |
|---|---|---|
| `legP-red-10dbae3c-esr8600000e-far-eq-elr.txt` | `10dbae3c` | `[FATAL_REGS] label=INSTRUCTION_ABORT cpu=0 spsr=0x80000305 esr=0x8600000e far=0xffff0000431ffc60 elr=0xffff0000431ffc60` — the #635 family's field signature, `FAR == ELR`, a kernel VA that is not kernel text, taken at EL1. Leg verdict `refused=0 ... fatal=1:FAIL`. |
| `legUK-red-f16c87a2-no-el0-arm.txt` | `f16c87a2` | `refused_sources=0x100` — the only refusal came from the kernel-restore path, an arm this leg is not aiming at; bits 6/7 (the EL0 arm) never set. `FAIL`. |
| `legUT-red-f16c87a2-no-el0-arm.txt` | `f16c87a2` | `refused_sources=0x200` — the pre-existing dispatch-time bad-context guard, source id 9. Bits 6/7 never set. `FAIL`. |
| `legFK-red-f16c87a2-el0-kernel-pc-8200000e.txt` | `f16c87a2` | `[INSTRUCTION_ABORT] FAR=0xffff00004121cff0 ELR=0xffff00004121cff0 ESR=0x8200000e IFSC=0xe from_el0=1` twice, then `Terminating PID 88 (SIGSEGV)` — #637's face: an EL0 thread resumed at a kernel address. `el0_asm_refused=0:el0_faults=2:FAIL`. |
| `legFT-red-f16c87a2-pcalign-el0.txt` | `f16c87a2` | `[PC_ALIGN] ELR=0x4b7 FAR=0x4b7 from_el0=1` — #633's face: an EL0 thread resumed at a small integer that is a live tid. `el0_faults=3:FAIL`. |

## Anti-vacuity: the harness is live, the value is what makes the difference

| file | commit | what it shows |
|---|---|---|
| `legP-disarmed-10dbae3c-antivacuity.txt` | `10dbae3c` | `opportunities=481:injected=0:refused=0:fatal=0:PASS` with `[BOOT_TESTS:PASS]`. The injection point was reached 481 times and the boot is clean. |
| `legP-disarmed-704eb771-antivacuity.txt` | `704eb771` | `opportunities=395:injected=0:refused=0` — the new EL1 window refuses nothing in a production boot. |
| `legUK-disarmed-704eb771-antivacuity.txt` | `704eb771` | `opportunities=37:injected=0:refused=0` — the new Rust EL0 arm refuses nothing in a production boot. |
| `legFK-disarmed-704eb771-antivacuity.txt` | `704eb771` | `opportunities=62:injected=0:refused=0` — the new assembly EL0 arm refuses nothing in a production boot. |

Zero production refusals across all three disarmed runs is also the PR's answer to "production
refusals within the documented rate": the rate measured here is zero.

## Green after

| file | what it shows |
|---|---|
| `legP-green-704eb771-refused.txt` | `refused_sources=0x4` (the IRQ epilogue), `fatal=0`, `[BOOT_TESTS:PASS]`, and the record `[RESUME_PC_REFUSED:source=irq-epilogue:...:pc=0xffff0000433ffe70:x29=0x1:x30=0xffff00004059580c:sp=0xffff0000433fff70:spsr=0x60000005:cpu=1]`. |
| `legZ-green-704eb771-refused.txt` | the same arm at `pc=0x0`. |
| `legUK-green-704eb771-el0-restore-refused.txt` | `refused_sources=0x40` — bit 6, `el0-restore`, the Rust producer arm. `el0_faults=0`. |
| `legUT-green-704eb771-el0-restore-refused.txt` | the same arm at the tid value. |
| `legFK-green-704eb771-asm-el0-refused.txt` | `el0_asm_refused=1`, `refused_sources=0x4` — the IRQ epilogue's EL0 arm, with 346 opportunities and no EL0 fault. |
| `legFT-green-704eb771-asm-el0-refused.txt` | `el0_asm_refused=1`, `refused_sources=0x8` — the syscall epilogue's EL0 arm. |

## Mutations, one at a time, each reverted before the next

| file | mutation | result |
|---|---|---|
| `mutation-1-el1-window-reverted-to-floor-legP-red.txt` | the IRQ epilogue's `RESUME_PC_EL1_OK` replaced by the old `elr >= KERNEL_VIRT_BASE` floor | leg P red: `esr=0x8600000e far=elr=0xffff000054265d10`, an address inside the kernel-stack pool that the floor admits and the window does not. Proves the window, not merely a bound, is what closes #635. |
| `mutation-2-asm-el0-arm-removed-legFK-red.txt` | `RESUME_PC_EL0_OK` deleted from all three epilogues | leg FK red: `ESR=0x8200000e ... from_el0=1`, #637's face, `el0_asm_refused=0`. |
| `mutation-3-rust-el0-producer-arm-removed-legUK-red.txt` | the `resume_pc_is_user_dispatchable` guard deleted from `restore_userspace_context_inline` | leg UK red on its own verdict — and the assembly EL0 arm catches the value downstream (`source=irq-epilogue:el=1`), which is the defence-in-depth the two arms are meant to give. |

Every serial here predates the `el` → `el0` key rename in the refusal line, so they spell the field
`el=<0|1>` with 1 meaning "the refused target was EL0". The rename landed after these were
recorded; the values are unchanged.
| `mutation-4-all-el1-arms-deleted-legZ-red-esr86000005.txt` | the EL1 admission deleted from all four assembly consumers | leg Z red: `esr=0x86000005 far=0x0 elr=0x0` — **byte-identical to #576's filed signature**. The epilogue arms are what keep that face from returning through the exception-return path. |

## What these serials do not show

* No leg exercises the sync epilogue's EL0 arm or the dispatch ERET's arms directly; leg P's
  mutation-1 run happens to exercise the sync epilogue's EL1 arm (`refused_sources=0x2`), and the
  remaining arms are covered only by the shared macro and by the structural ratchet that every
  consumer invokes it. Stated rather than implied.
* No serial here names the producing store. PR-A converts four silent admissions into named
  refusals and stops them being fatal; it does not identify what writes the value. That is the
  next PR's subject.

---

# Round 2 (ruling R46) — the ret-dispatch arm, exercised for the first time

Round 1 disclosed (D7) that the ret-dispatch assembly net had never been fired by a leg. R46 ordered
forced refusal legs for it, and firing it found two defects and proved the round-2 fix. These runs
used the same QEMU line as the round-1 legs, `-cpu cortex-a72`, 60 s, kernel built with
`--features boot_tests,ret_floor_oracle` (leg F is the pre-existing `ret_floor_oracle`: it replaces
one ret-dispatch resume PC with `0x0100_0000` for a designated kthread victim, which the unified EL1
window refuses at `aarch64_ret_to_kernel_context`, source `ret-dispatch`).

| file | what it shows |
|---|---|
| `legF-r2-red-stale-owner-canary-wrong-tid-fatal.txt` | **Red, first ever firing of the arm.** `[RET_FLOOR_ORACLE:...FIRED:tid=10]` but `[RESUME_PC_REFUSED:source=ret-dispatch:...:tid=4:...]` — the refusal drain read the OWNER-TID canary, which the two ret-dispatch sites never stamped, so it named the last ERET-dispatched thread instead of the refused one. The drain terminated and dequeued tid 4, an innocent thread, and the boot died: `[DATA_ABORT] FAR=0x8010000 ELR=0xffff00004040af94 ESR=0x96000010 DFSC=0x10`, resolved against this build's own ELF to `Scheduler::schedule_deferred_requeue+0x6b0`. This is review note N1 ("a kill decision keyed on an unproven identity") firing, not dormant. |
| `legF-r2-green-pivot-and-canary.txt` | **Green after both round-2 fixes.** `[RESUME_PC_REFUSED:source=ret-dispatch:...:tid=10...]` — the canary now names the refused thread, because both ret-dispatch sites stamp it. `[RESUME_PC_CUSTODY:...:record_slot=5:drain_slot=-1:on_refused_stack=0:...:OK]` — the drain (and the reclamation that follows it in the same `run_deferred_reclamation` call) is running on the CPU-owned idle stack, not on the refused thread's pool slot. `[RET_FLOOR_ORACLE:...:refused_tid=10:ret_dispatch_arm=1:custody_checks=1:custody_blind=0:fatal=0:PASS]`, `[BOOT_TESTS:PASS]`. |
| `legF-r2-mutation-M5-nopivot-red-custody-blind.txt` | **The designated mutation for the B3 fix**: the single `mov sp, <idle_stack_top>` deleted from `RESUME_PC_RECORD_NOFRAME`, nothing else changed. `[RESUME_PC_CUSTODY:...:record_slot=5:drain_slot=5:on_refused_stack=1:...:named_live=0:STACK_CUSTODY_BLIND]` — the drain is executing on the refused thread's own pool slot while the two per-CPU words that are the entire `is_kernel_stack_slot_live()` predicate name a different stack, i.e. the over-free window is open, and it is about to terminate that thread and hand the slot to the reaper. `custody_blind=1 ... FAIL`. |
| `legF-r2-disarmed-antivacuity.txt` | Harness live, store skipped: `armed=1:fired=0:opportunities=30:refused=0:custody_checks=0:...:PASS` with `[BOOT_TESTS:PASS]`. 30 opportunities and zero production refusals on this arm — the green above is not a dead harness. |

## The leg-P regression this round found in its own first fix, and removed

An intermediate round-2 attempt gated the drain's terminate on "the refusal record's SP lies inside
the victim thread's kernel stack". That is wrong for an EL1 frame taken while a thread runs on a
per-CPU stack (inline-schedule trampoline / scheduler stack), where the frame is legitimately not on
the thread's pool stack.

| file | what it shows |
|---|---|
| `legP-r2-control-14272b9a-green.txt` | Control: leg P at `14272b9a`, the pre-round-2 tree, `fatal=0:PASS` (2/2 boots). |
| `legP-r2-red-identity-gate-regression.txt` | The gate in place: `refused=2:refused_sources=0x4 ... fatal=1:FAIL`, 3/3 boots, with `[RESUME_PC_CUSTODY:...record_slot=-1...]` showing the refused frame on a per-CPU stack. |
| `legP-r2-green-after-gate-removed.txt` | Gate removed, canary stamp kept: `fatal=0:PASS`, `[BOOT_TESTS:PASS]`. The identity defect is fixed at its source (the unstamped canary), and leg F asserts it directly with `refused_tid == victim_tid`. |

The round-1 statement that no leg exercises the ret-dispatch arm no longer holds: leg F does, in
both directions, with a mutation. The dispatch ERET's own arms remain covered only by the shared
macro and the structural ratchet.

## Round 3 (ruling R47) — the drain hazard cluster

Firing the ret-dispatch arm in round 2 made the *refusal drain* reachable for the first time. Round 3
fixes three hazards inside it. Every serial below was captured on the shipped round-3 tree.

### (a) foreign records are report-only — leg G

`drain_asm_resume_pc_refusals` used to iterate every `MAX_CPUS` record and terminate
`last_dispatched_tid(record_cpu)` for any of them. That word is the *publishing* CPU's OWNER-TID
canary; read across CPUs it is a race. Leg G plants a synthetic record in an **offline** per-CPU slot
(`MAX_CPUS-1`; the gate boots `-smp 4`), so the record is foreign by construction and cannot perturb
any running CPU's own record or canary, and stamps that slot's canary with a dedicated live kthread.

| file | what it shows |
|---|---|
| `legG-r3-green-foreign-report-only.txt` | **Green.** `[RESUME_PC_CUSTODY:drain_cpu=1:record_cpu=7:record_elr=0x2000000:record_sp=0x0:record_count=1:FOREIGN_REPORT_ONLY]` — described, not consumed. `[RESUME_PC_FOREIGN_ORACLE:...:foreign_reports=1:record_still_published=1:canary_present=1:canary_terminated=0:canary_progress=29:PASS]`: the record is still published at report time (left for its owner) and the thread it named is alive and running. |
| `legG-r3-mutation-M6-act-on-foreign-red-canary-killed.txt` | **The designated mutation**: the foreign early-out condition replaced by `if false`, nothing else. CPU 3 claims CPU 7's record and terminates the canary — `foreign_reports=0:record_still_published=0:canary_present=0`, `canary_progress` frozen at 3 (vs 29 green) — and the custody line shows it did so **while executing on that canary's own kernel stack**: `[RESUME_PC_CUSTODY:drain_cpu=3:record_cpu=7:victim_tid=10:...:on_victim_stack=1:...]`. That is the over-free, made visible. `FAIL`. |
| `legG-r3-disarmed-antivacuity.txt` | Harness live, nothing planted: `armed=1:planted=0:foreign_reports=0:record_still_published=0:canary_present=1:canary_progress=30:PASS`. The green above is not a dead harness. |

A **production-shaped** foreign record also appears in `legP-r3-green-guard-events-explains-coalescing.txt`:
`[RESUME_PC_CUSTODY:drain_cpu=2:record_cpu=3:...:FOREIGN_REPORT_ONLY]`, followed moments later by CPU
3 draining its own record with the correct `victim_tid=10`. Not planted — the real cross-CPU race,
observed and correctly left alone.

### (b) the record publishes its source behind a store barrier

Both macros wrote `source` last as the validity word with a plain `str`. Both now do `dmb ishst`
first, on the cold refusal arm only. **This has no observational leg** — QEMU TCG does not reorder
stores, so nothing can redden from a missing barrier, and saying otherwise would be theatre. It is
proven structurally: `aarch64_resume_pc_records_publish_their_source_behind_a_store_barrier` asserts
the barrier sits after every payload store and before the source store in both macro bodies, and
mutation **M8** (`pr3-mutation-patches/M8-no-release-barrier.patch`, both `dmb ishst` lines deleted)
turns that ratchet red: 70 passed, 1 failed.

### (c) the CPU stops publishing a thread it is about to have reclaimed — leg F

The refused dispatch had already published its victim as this CPU's current thread. After the round-2
fix the refused stack slot is no longer named live, so nothing holds reclamation back — marking the
victim Terminated with that publication standing leaves `current_thread_ptr` aimed at a `Box` the
next reclaim pass may drop. The drain now repoints the bookkeeping at idle **before** the terminate,
and counts both the hazard and the repair.

| file | what it shows |
|---|---|
| `legF-r3-green-final-tree-repoint.txt` | **Green on the shipped tree.** `[RESUME_PC_CUSTODY:...:current_repointed=1:OK]` and `[RET_FLOOR_ORACLE:...:refused_tid=10:custody_checks=1:custody_blind=0:current_dangling=1:current_repointed=1:fatal=0:PASS]`. `current_dangling=1` is the anti-vacuity half — the hazard genuinely occurs in this leg; `current_repointed == current_dangling` is the fix. |
| `legF-r3-mutation-M7-no-current-repoint-red.txt` | **The designated mutation for (c)**: the two repoint actions deleted, the hazard counter kept. `current_dangling=1:current_repointed=0:FAIL`, `[BOOT_TESTS:FAIL:1]`. |
| `legF-r3-mutation-M5-nopivot-red-custody-blind.txt` | M5 re-run on the round-3 tree: still red, still for the round-2 reason. `on_refused_stack=1:named_live=0:STACK_CUSTODY_BLIND`, `custody_blind=1:FAIL`. |
| `legF-r3-disarmed-antivacuity-final-tree.txt` | **R47 item 1.** The round-2 disarmed serial was superseded — it carried an `identity_mismatches` field that existed only in an intermediate build. Re-run on the shipped tree: `armed=1:fired=0:opportunities=33:refused=0:refused_tid=0:custody_checks=0:custody_blind=0:current_dangling=0:current_repointed=0:fatal=0:PASS` with `[BOOT_TESTS:PASS]`. 33 opportunities, zero production refusals. |

### review note N8 — leg P's arithmetic is now in its own verdict

| file | what it shows |
|---|---|
| `legP-r3-green-guard-events-explains-coalescing.txt` | `[RESUME_PC_EL1_ORACLE:...:opportunities=3:injected=3:refused=2:guard_events=3:...:PASS]`. `injected` counts injections, `refused` counts *drained* records, and the per-CPU record slot holds one entry — two refusals between drains coalesce. `guard_events` is the per-arm execution count, summed across CPUs, and it does not coalesce: 3 arms fired for 3 injections, 2 records were drained. The gap is explained by the data instead of by a deviation note. |

### mutation patches

The four round-2/round-3 mutation patches are preserved verbatim in
`../pr3-mutation-patches/` (`M5-no-sp-pivot`, `M6-act-on-foreign`, `M7-no-current-repoint`,
`M8-no-release-barrier`). Each applies and reverts cleanly with `git apply` against this branch, and
each mutant compiles — so every red above can be reproduced exactly.
