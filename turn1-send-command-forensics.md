# Turn 1/2 - VirGL send_command near-null deref forensics

## A. Branch state

- Repository: `/Users/wrb/fun/code/breenix`
- Base: `main` and `origin/main` both at `57bf6c4f`.
- Pre-branch status: clean; `origin/main..HEAD` was empty.
- Branch created: `fix/virgl-send-command-deref`.
- Scope honored: no source edits and no new Parallels boots. This document is the only project artifact from the turn.

## B. Capture A summary - Turn 13 Run 5

Path:
`/Users/wrb/Downloads/Ralph/breenix-scheduler-mutex-leak-1779100838/turn13-artifacts/reproduce-run5/`

- Classification/result: `classification=non-silent-failure`; `run_rc=still-running-at-capture`; `gdb_rc=0`.
- Fault: first abort at serial line 724: `FAR=0xccd ELR=0xffff000040104484 ESR=0x96000005 DFSC=0x5 TTBR0=0x1000044069000 from_el0=0 cpu=0`.
- BWM frame at fault: `tid=13 name=bwm`, `x30=0xffff0000401044cc`, `sp=0xffff000054254550`, `x19=0xffff000040231000`, `x20=0x19e75`, `x8=0xed0e`, `x9=0x197be1f3b4cb396`.
- Later softlock thread dump frame: `tid=13 state=D user pid=3 elr=0xffff000040104470 x30=0xffff0000401044cc sp=0xffff000054254550`; no saved_lr was available for this D-state frame.
- Immediate serial context: BWM was still rendering immediately before the abort: `virgl-composite Frame #20000`, heartbeat `uptime_ms=105324`, `bwm-fps` 144, then the abort.
- Last pre-abort freeze-watch line: `uptime_ms=105330 submits=60376 completes=60378 fails=0 last_completion_ms=105316 fps_last_5s=193`, `cur_cpu0..7=0,0,0,0,0,0,0,0`, `total_threads=0 blocked_threads=0`, `sched_lock=busy`, `gpu_pci_lock=busy`.
- Endpoint GDB lock/hardware state: `scheduler_lock_byte=0`, `gpu_pci_lock_byte=1`, `ahci_irq=0`, `ahci_isr_count=0`.
- First post-abort softlock: ready queue length 0, total threads 16, blocked threads 7, per-CPU current threads `0,4,5,6,7,8,9,10`; BWM remained `state=D`.
- Screenshot: Bounce window was still visible with animated balls and `FPS: 228`, visually confirming the BWM workload was the foreground activity at failure.

## C. Capture B summary - Turn 15 Run 4

Path:
`/Users/wrb/Downloads/Ralph/breenix-scheduler-mutex-leak-1779100838/turn15-artifacts/reproduce-run4/`

- Classification/result: `classification=no-silent-freeze-within-window`; `run_rc=still-running-at-capture`; `gdb_rc=0`.
- Fault: first abort at serial line 1056: `FAR=0xccd ELR=0xffff000040104484 ESR=0x96000005 DFSC=0x5 TTBR0=0x1000044069000 from_el0=0 cpu=0`.
- BWM frame at fault: `tid=13 name=bwm`, `x30=0xffff0000401044cc`, `sp=0xffff000054254550`, `x19=0xffff000040231000`, `x20=0x38b25`, `x8=0xb4af`, `x9=0x397bc08334f112b`.
- Later softlock thread dump frame: `tid=13 state=D user pid=3 elr=0xffff00004010447c x30=0xffff0000401044cc sp=0xffff000054254550`; no saved_lr was available for this D-state frame.
- Immediate serial context: BWM was active immediately before abort: `virgl-composite Frame #37000`, heartbeat `uptime_ms=231465`, `bwm-fps` 165, then the abort.
- Last pre-abort freeze-watch line: `uptime_ms=230446 submits=110954 completes=110957 fails=0 last_completion_ms=230446 fps_last_5s=160`, `cur_cpu0..7=0,3,5,6,7,8,9,10`, `total_threads=16 blocked_threads=5`, `rq_total=1`, `sched_lock=ok`, `gpu_pci_lock=ok`.
- Later/result freeze-watch at capture: `uptime_ms=250460 submits=111789 completes=111791 fails=0 last_completion_ms=232227 fps_last_5s=0`, `cur_cpu0..7=0,4,5,6,7,8,3,10`, `total_threads=16 blocked_threads=6`, `sched_lock=ok`, `gpu_pci_lock=busy`.
- Endpoint GDB lock/hardware state: `scheduler_lock_byte=0`, `gpu_pci_lock_byte=1`, `ahci_irq=0`, `ahci_isr_count=0`.
- First post-abort softlock: ready queue length 0, total threads 16, blocked threads 7, per-CPU current threads `0,4,5,6,7,8,9,10`; BWM remained `state=D`.
- Screenshot: Bounce window was still visible with animated balls and `FPS: 207`, again confirming active BWM foreground workload at failure.

## D. Common pattern

Identical across both captures:

- Same hard abort signature: `FAR=0xccd`, `ELR=0xffff000040104484`, `ESR=0x96000005`, `DFSC=0x5`, `from_el0=0`, `cpu=0`.
- Same victim: BWM `tid=13`, kernel-mode data abort inside `kernel::drivers::virtio::gpu_pci::send_command`.
- Same architectural frame: `sp=0xffff000054254550`, `x30=0xffff0000401044cc`.
- Same lock envelope after capture: endpoint `scheduler_lock_byte=0`, `gpu_pci_lock_byte=1`, `ahci_irq=0`, `ahci_isr_count=0`.
- Same important register clue: `x19=0xffff000040231000`, the correct static page base for `GPU_PCI_STATE`, while the fault address proves `x17=0x1` at the fault.
- GPU progress was healthy before the abort: submits/completes kept advancing, `fails=0`, and FPS was nonzero.
- Later softlock dumps show BWM stuck in `state=D` and the ready queues empty, but that is downstream of the first data abort.

What varies:

- Time to first abort: about 105.3s in Capture A, about 231.5s in Capture B.
- The saved ELR in later softlock snapshots differs by one instruction (`0x40104470` vs `0x4010447c`), both inside the same polling loop immediately before the faulting `ldrh`.
- Capture A's last freeze-watch sample was taken during a scheduler-lock/gpu-lock busy window with all `cur_cpu` fields sampled as 0; Capture B's last pre-abort sample still had a normal scheduler view and GPU lock `ok`.
- Capture B continued long enough to capture later frozen BWM progress at `uptime_ms=250460` with `fps_last_5s=0`.

## E. send_command code analysis

`gpu_pci.rs` uses a global `GPU_PCI_STATE: Option<GpuPciDeviceState>`, whose `last_used_idx: u16` tracks the last consumed VirtIO control-queue used-ring index. The 2-descriptor command path in `send_command` compares the device-written used-ring index against this saved `last_used_idx`:

```rust
let used_idx = unsafe {
    let q = &raw const PCI_CTRL_QUEUE;
    read_volatile(&(*q).used.idx)
};
if used_idx != state.last_used_idx {
    state.last_used_idx = used_idx;
    return Ok(());
}
```

Disassembly of the matching kernel (`target/aarch64-breenix/release/kernel-aarch64`) maps the fault precisely:

```text
ffff000040103d80: adrp x17, 0xffff000040231000
...
ffff000040104470: dc civac, x24
ffff00004010447c: dmb ishld
ffff000040104480: ldrh w21, [x24, #0x2]
ffff000040104484: ldrh w8,  [x17, #0xccc]   ; faulting instruction
ffff000040104488: cmp  w21, w8
```

The addresses identify the two operands:

- `x24 = 0xffff00004024d000`, so `[x24,#0x2] = 0xffff00004024d002`, which is `PCI_CTRL_QUEUE.used.idx` (`PCI_CTRL_QUEUE` is at `0xffff00004024c000`, used ring begins at +0x1000, `idx` is +2).
- `x17` is supposed to be the page base `0xffff000040231000`; `[x17,#0xccc] = 0xffff000040231ccc`, which is inside `GPU_PCI_STATE` (`GPU_PCI_STATE` symbol at `0xffff000040231ba8`) and corresponds to `state.last_used_idx`.

So offset `0xccc` is not a VirtIO MMIO register, not a notify offset, and not a queue-ring offset by itself. It is the compiler's static-data access to `GPU_PCI_STATE.last_used_idx`. The actual queue register/ring access in this compare is the adjacent `PCI_CTRL_QUEUE.used.idx` read through `x24 + 0x2`.

The polling loop does not reload `x17` each iteration. It loads the page base once at function entry with `adrp x17, 0xffff000040231000`, then reuses that caller-saved register across the long spin loop. The compiler saves the correct page base in callee-saved `x19` around the inlined time/division path (`mov x19, x17` before `bl __udivti3`, then `mov x17, x19` after the call). Both captures show `x19=0xffff000040231000` at fault, but the effective fault address `0xccd` proves `x17=0x1`.

## F. Hypothesis table

| Hypothesis | Evidence for | Evidence against | How to test |
|---|---|---|---|
| H1: caller-saved `x17` is clobbered across kernel interrupt/preemption while `send_command` spins | Both captures fault at the same `ldrh w8, [x17,#0xccc]`; FAR `0xccd` implies `x17=0x1`; `x19` still holds the correct `0xffff000040231000` page base at fault; disassembly shows `x17` loaded once and reused across a long polling loop; BWM later remains D-state holding `gpu_pci_lock`. This matches the earlier solved stale-volatile-register class, but in the GPU polling loop rather than ret-dispatch. | We do not yet have a live GDB capture of `x17`, only the FAR-derived value. It also implies a broader kernel-preemption invariant risk, not only a GPU driver bug. | Patch or probe `send_command` so the `last_used_idx` address is not kept in caller-saved `x17` across the spin loop, then disassemble to confirm no long-lived `x17` base and run the 5-boot Parallels stress. Optional targeted capture: record `x17`/`x19` or a postmortem atomic when `FAR=0xccd` before changing code. |
| H2: `GPU_PCI_STATE` memory was overwritten and `last_used_idx` moved/corrupted | The faulting access is into `GPU_PCI_STATE`, and both failures happen under sustained BWM GPU traffic. A memory overwrite near the static state could make driver metadata nonsensical. | A corrupted memory value would affect the data read from `0xffff000040231ccc`, not normally make the address register become `0x1`. The correct page base survives in `x19`; the symbol address and layout are stable across captures. | Add a non-serial invariant around the global state address/nearby canary, or use GDB to inspect `GPU_PCI_STATE` and adjacent statics after a fail. |
| H3: `PCI_CTRL_QUEUE.used` pointer/ring memory became invalid | The source polling loop is waiting on the VirtIO used ring, and the hard failure occurs in the used-index compare. | The faulting instruction is the `state.last_used_idx` read, not the used-ring read. The used-ring read at `[x24,#0x2]` immediately precedes the fault and succeeds; `x24` is the correct `PCI_CTRL_QUEUE.used` base. | If failures persist after H1 fix, capture `x24`, used ring physical/virtual addresses, and queue descriptor state at the same fault site. |
| H4: cached notify address or MMIO BAR pointer became stale | VirtIO PCI notify/cache code exists, and the high-level objective expected an MMIO/queue pointer stale failure. | `notify_queue_fast(0)` has already run before this polling-loop fault. The faulting address is not the notify BAR and not in `VirtioPciDevice.cached_notify_addrs`; the instruction is a static-data read. | Instrument or GDB-check `state.device.cached_notify_addrs[0]`, `notify.virt_base`, and common/notify BARs only if H1 fix does not eliminate the class. |
| H5: speculative `last_used_idx` advancement or wraparound confused completion polling | The driver has stale-completion drain and timeout recovery logic around `last_used_idx`, and BWM runs enough commands to exercise wraparound. | Bad index arithmetic can cause timeouts or false completion, but it does not explain FAR `0xccd`; the pointer register is wrong before the value can be compared. | Add trace-buffer counters for `diff`, `last_used_idx`, and `used_idx` around drain/complete after the register-clobber fix. |

## G. Verdict

Best-supported hypothesis: **H1, `send_command` keeps the static `GPU_PCI_STATE.last_used_idx` page base in caller-saved `x17` across a long kernel polling loop, and `x17` is clobbered to `0x1` before the next poll iteration.**

The decisive evidence is the x17/x19 split. The fault address `0xccd` means the CPU tried to execute `*(u16 *)(0x1 + 0xccc)`. At the same time, the abort dump shows `x19=0xffff000040231000`, exactly the page base `x17` should have held. The disassembly explains why: the compiler uses `x17` as the long-lived static page base and also saves it in callee-saved `x19` around the inlined time/division path. The correct value surviving in `x19` while `x17` is bad is much more consistent with volatile-register clobber across interrupt/preemption than with a freed MMIO mapping, bad queue pointer, or static memory overwrite.

This narrows the root cause name to: **caller-saved register lifetime violation in the 2-desc VirtIO GPU `send_command` completion polling loop, specifically for the hoisted `GPU_PCI_STATE.last_used_idx` base register.**

## H. Turn 2/3 proposal

Proceed to a straight fix unless Claude wants one extra confirmatory GDB turn. The first fix should stay narrow in `kernel/src/drivers/virtio/gpu_pci.rs`:

1. Refactor the 2-desc `send_command` loop so the compare does not rely on a long-lived caller-saved `x17` static page base. Candidate: snapshot `state.last_used_idx` into a local that the compiler keeps in a callee-saved register or stack slot, compare `PCI_CTRL_QUEUE.used.idx` against that local inside the loop, and write `state.last_used_idx` only on completion/timeout. Verify by disassembly before booting.
2. If the compiler still emits a long-lived caller-saved static base across the loop, use a tiny helper or explicit reload pattern for `state.last_used_idx` and verify the generated code again.
3. Run the standard zero-warning build gate.
4. Run the contract's 5-boot Parallels stress gate. Success criteria: 5/5 pass, 0 `FAR=0xccd`, BWM progresses, no SOFT_LOCKUP.

If Claude prefers targeted capture first, the highest-value capture is not serial logging. It is a GDB or postmortem-readable register snapshot at `ELR=0xffff000040104484` confirming `x17=0x1` and `x19=0xffff000040231000`, plus a memory read of `0xffff000040231ccc`.
