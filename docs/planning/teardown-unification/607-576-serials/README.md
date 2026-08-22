# Preserved serials — the aarch64 zero-PC-jump family (#576, #607, and the exec-commit hole)

Every serial here was recorded by booting a kernel built from a named commit on
`fix/607-576-zero-pc-family` under

```
qemu-system-aarch64 -M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 4 \
  -kernel <kernel> -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 \
  -drive if=none,id=ext2,format=raw,file=<copy of target/ext2-aarch64.img>,throttling.iops-total=2000 \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 -serial file:<serial>
```

for 60 s, always with the soft-float kernel target `aarch64-breenix-kernel.json`.

Two commits are referenced throughout:

* **`c3b0133d`** — the test commit. The three feature-gated self-tests exist; neither production
  change does. This is what "before" means.
* **`6b3e143d`** — branch HEAD. Both production changes and the gate/ratchet update are in.

## Leg K — `--features boot_tests,ret_zero_pc_oracle`

The self-test zeroes `context.x30` for exactly one designated, disposable kernel thread, once, under
the scheduler lock, immediately before the ret-based kernel dispatch reads that field.

| file | commit | what it shows |
|---|---|---|
| `legK-red-testcommit-c3b0133d-esr8600000d.txt` | `c3b0133d` | `[RET_ZERO_PC_ORACLE:…:refused=0:refused_tid=0:FAIL]` and `[FATAL_REGS] label=INSTRUCTION_ABORT cpu=2 spsr=0x80000305 esr=0x8600000d far=0x0 elr=0x0` — an EL1 fetch at VA 0 with the kernel TTBR0 root live |
| `legK-green-head-6b3e143d-refused.txt` | `6b3e143d` | `[RET_ZERO_PC_ORACLE:…:refused=1:refused_tid=10:PASS]`, no abort, `[BOOT_TESTS:PASS]`, and the census line `[RET_DISPATCH_REFUSED:tid=10:cpu=3:x30=0x0:elr=0xffff00004045c630:sp=0xffff000054265ef0:has_started=1:bis=1:priv=1]` |

The red serial's `esr=0x8600000d far=0x0 elr=0x0` is **byte-identical to #626's filed header**,
produced here from a single-field write with a kernel root installed. That is the constructed proof
that the `IFSC=0xd` vs `IFSC=0x5` fork between #626 and #576 is a read-out of which TTBR0 was live at
the transfer, not a fork in the mechanism. It is not a claim about #626, whose own capture carries
`spsr=0x200023c5` with DAIF `I`+`F` set and therefore was not reached from any dispatch transfer —
every resume-PC consumer clears those bits one instruction earlier.

Note that this red boot carries the abort only in its `[FATAL_REGS]` record; the
`[INSTRUCTION_ABORT] FAR=… ELR=… ESR=…` header line did not survive. The service-sequence gate's
`instruction_abort_signatures()` reads both record forms and unions them, so the boot still
classifies field-exactly.

## Leg E — `--features boot_tests,ret_zero_pc_oracle_exec`

The self-test marks exactly the first `ExecSchedCommit::apply` subject inline-saved and started, once,
inside the lock hold that commit already takes, immediately before it replaces `t.context`. The
context `apply()` installs is an ERET-shaped first-entry context with `x30 == 0`, carrying the freshly
installed **user** TTBR0 — #576's field set rather than #626's.

| file | commit | what it shows |
|---|---|---|
| `legE-red-testcommit-c3b0133d-inline-left-set.txt` | `c3b0133d` | `[EXEC_COMMIT_DISARM_ORACLE:…:victim_tid=1212:inline_left_set=1:cleared=0:FAIL]` — the commit left the thread classified inline-saved while installing a zero resume PC |
| `legE-green-head-6b3e143d-cleared.txt` | `6b3e143d` | `[EXEC_COMMIT_DISARM_ORACLE:…:inline_left_set=0:cleared=1:PASS]` |

**Scope, stated plainly:** on the unrepaired tree this leg's deterministic red is the *classification*
being left set. In the boots recorded here the mis-classified thread did not go on to be
ret-dispatched to 0 before something else cleared it, so the second step of that predicted chain is
not demonstrated by these serials and is not claimed.

## Leg S — `--features boot_tests,strand_inject_live_outgoing` (#607)

The self-test widens the existing one-shot injection so it also engages when the outgoing thread is a
dedicated, no-wakeup driver thread, which forces the `scheduler_ptr`-null fallback arm of
`inline_schedule_trampoline` to own that thread's outgoing transaction.

| file | commit | what it shows |
|---|---|---|
| `legS-red-testcommit-c3b0133d-stranded1.txt` | `c3b0133d` | `[STRAND_LIVE_OUTGOING_ORACLE:…:fired=1:live_outgoing=1:outgoing_tid=10:stranded=1:FAIL]` — the abandoned outgoing thread dwells past `STRAND_DWELL_MS` |
| `legS-notfired-testcommit-c3b0133d-antivacuity.txt` | `c3b0133d` | `…:fired=0:live_outgoing=0:stranded=0:FAIL` — a boot where the one-shot never engaged still reports FAIL rather than passing by omission |
| `legS-green-head-6b3e143d-stranded0.txt` | `6b3e143d` | `…:fired=1:live_outgoing=1:outgoing_tid=10:stranded=0:PASS` |

Observed rate at the test commit: 2 of 3 boots `stranded=1:FAIL`, 1 of 3 `fired=0:FAIL`. At HEAD:
3 of 3 `fired=1 … PASS`.

**Disclosed:** under this feature only, the widened arm clears `previous_thread` for the CPU when the
stimulus engages, because `fix_exception_cleanup_cpu_state()` would otherwise opportunistically
re-enqueue the dropped thread and mask the very transaction under test. That backstop is
opportunistic, not guaranteed — #607 was observed in the field at 1/50 *despite* it — so suppressing
it isolates the fallback arm's own completion. It also means this leg measures that completion, not
the end-to-end field rate.

## Control

`control-disarmed-testcommit-9bd703ba.txt` — the test commit (at its pre-rebase SHA `9bd703ba`, same
content) built with `--features boot_tests` and **no** self-test feature: 0 aborts, 0
`[TEST:…:FAIL:…]` lines, `[BOOT_TESTS:PASS]`. The stimuli manufacture no unrelated collateral. The one
extra `[TEST:process:kernel_stack_ownership_oracle:FAIL:…]` line that appears in leg K's red boot is
the direct consequence of that leg's own designated victim dying.

## Mutations — both safety nets are load-bearing, for different things

Taken at HEAD's pre-rebase SHA `756c6cbb` (same production content as `6b3e143d`), in a scratch
worktree, applied one at a time and reverted.

| file | mutation | result |
|---|---|---|
| `mutation-A-rust-refusal-removed-asm-floor-kept.txt` | the Rust `resume_pc_is_dispatchable` refusal removed from `take_inline_ret_dispatch_info`; the assembly floor kept | `refused=0 … FAIL`, **no** abort, `[BOOT_TESTS:PASS]` — the assembly floor contains the transfer, and the Rust arm is what names the producer |
| `mutation-B-both-safety-nets-removed.txt` | mutation A plus removing the `cmp x1, #0x1000 / b.hs / adrp x1, idle_loop_arm64 / add …` floor from `aarch64_ret_to_kernel_context` | `refused=0 … FAIL` **and** `[INSTRUCTION_ABORT] FAR=0x0 ELR=0x0 ESR=0x8600000d IFSC=0xd TTBR0=0x40200000 from_el0=0`, `[BOOT_TESTS:FAIL:1]` |

## Related prior evidence, not copied here

`docs/planning/teardown-unification/609-serials/pcalign-elr4b5-layouterror-cortexa72-ssgate-boot1.txt`
and `.../instrabort-esr8600000d-ifsc0d-max-ssgate-boot7.txt` carry the `[PC_ALIGN] ELR=<tid>` face —
`0x4b5 = 1205` and `0x4b7 = 1207`, each the just-spawned child's tid, on the same CPU 2 at the same SP
`0xffff000054286420` at the same spawn phase. That face is filed as **#633** and is not claimed by
this PR. The second of those two serials is also #626's own capture.
