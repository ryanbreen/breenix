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
