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

---

# Round 2 — the garbage-kernel-PC face (#635)

Round 1 closed the `PC == 0` face and left the `PC == <some other kernel address>` face of the same
consumer fail-open: its predicate admitted the whole upper half of the address space and its assembly
floor admitted anything at or above `0x1000`, so a kernel **stack** address passed both. The round-1
gate then produced exactly that, 3 times byte-identically on two CPU profiles. Round 2 narrows the
predicate to the kernel text window and raises the assembly floor to the same lower bound.

Two commits are referenced below:

* **`972a0832`** — the round-2 fix. Text-range predicate, raised assembly floor, `:pc=` in the
  refusal record, and the two new self-test legs.
* The mutations are temporary edits on top of `972a0832`, applied one at a time in a scratch
  worktree, measured, and reverted. Nothing measured here is committed except the serials.

Boot recipe is unchanged from the sections above (`-cpu cortex-a72 -smp 4`, 60 s, soft-float target).

## The field face this round is about

Three clean-gate boots on `fix/607-576-zero-pc-family` @ `c9c75c3b`, preserved here as the reason the
round exists:

| file | profile | record |
|---|---|---|
| `gate-clean100-cortexa72-boot3-stackpc-8600000e.txt` | cortex-a72 | `FAR=0xffff000054243f00 ELR=0xffff000054243f00 ESR=0x8600000e IFSC=0xe`, `spsr=0x20000305`, `x29 == x30 ==` the same value |
| `gate-clean100-cortexa72-boot37-stackpc-8600000e.txt` | cortex-a72 | byte-identical |
| `gate-clean100-max-boot30-stackpc-8600000e.txt` | max | same, `spsr=0x20002305` (bit 13 `ALLINT`, which `max` implements and `cortex-a72` does not; DAIF and mode identical) |
| `gate-starved100-max-boot3-disagreeing-pair-613.txt` | max | **not** this face — a disagreeing record *pair*, filed as #613; kept here because its second record shares the ESR/IFSC at different addresses, and because it carries the EL1 `[PC_ALIGN] ELR=0x4b1` record appended to #633 |

Filed as **#635**. The producer stays open there; this round closes the consumer.

## Leg T — `--features boot_tests,ret_stack_pc_oracle` — #635's face by construction

The self-test writes the designated victim thread's own saved `sp` into `context.x30`, once, under
the scheduler lock, at the same hook leg K uses. That is a value the round-1 predicate accepts and
the round-2 predicate refuses.

| file | tree | what it shows |
|---|---|---|
| `mutation-1-rust-bound-reverted-legT-red.txt` | `972a0832` + the Rust bound reverted to its round-1 body, assembly floor left raised | `[RET_STACK_PC_ORACLE:…:refused=0:refused_tid=0:FAIL]` and `[INSTRUCTION_ABORT] FAR=0xffff000054265f00 ELR=0xffff000054265f00 ESR=0x8600000e`, `[FATAL_REGS] cpu=1 spsr=0x20000305 … x30=0xffff000054265f00`, `x16=0xffff000040400000` |
| `legT-green-stack-pc-refused.txt` | `972a0832` | `[RET_DISPATCH_REFUSED:tid=10:pc=0xffff000054265f00:…]`, `[RET_STACK_PC_ORACLE:…:refused=1:refused_tid=10:PASS]`, no abort |
| `legT-green-repeat1-no-collateral.txt`, `legT-green-repeat2-no-collateral.txt` | `972a0832` | the same green, twice more, and `[BOOT_TESTS:PASS]` |

The red boot's field set — `ESR=0x8600000e`, `IFSC=0xe`, `FAR == ELR == x30`, `spsr=0x20000305`,
`from_el0=0` — is #635's, produced from a single-field write. **Scoped honestly:** the field capture
additionally has `x29 == x30` and `sp` 0x2c0 below the PC, because there a whole shifted region was
restored into the callee-saved file; this construct writes one field, so its `x29` and `sp` differ.
What is reproduced is the transfer and its resulting fault, not the producer's write pattern.

`x16=0xffff000040400000` in that dump is `__kernel_text_start` — the raised assembly floor loading
its bound, and correctly letting the value through, since a stack address is above kernel text. That
is the direct evidence that the assembly floor cannot catch this face and the Rust predicate must.

**Collateral, disclosed:** 1 of the 3 green boots (`legT-green-stack-pc-refused.txt`) also carries
`[TEST:process:kernel_stack_ownership_oracle:FAIL:ownership stress slot allocation/free equality
failed]` with `slot_alloc_delta=1000:slot_free_delta=1001:slot_balance=-1` — one *more* free than
alloc inside the census window, which is what a thread allocated before the window and reaped inside
it looks like; the leg's own designated victim is exactly such a thread, and its verdict line lands
between that test's `START` and its measurement. The same boot reads `two_owner=0`, `zero_owner=0`,
`drop_refused_live=0`, `pte_overwrite_refusals=0`, `frame_balance=0` — no double ownership, no
over-free of a live slot. The other 2 of 3 read `slot_free_delta=1000:slot_balance=0` and
`[BOOT_TESTS:PASS]`, and the disarmed control at the same commit is clean, so this is the stimulus
perturbing that test's window, not a property of the fix.

## Leg F — `--features boot_tests,ret_floor_oracle` — what the assembly floor alone is worth

The assembly floor exists for the window *between* the Rust predicate and the branch, so this leg
substitutes `resume_pc = 0x0100_0000` at the two call sites of `aarch64_ret_to_kernel_context`, after
the predicate has already accepted the real value. `0x0100_0000` is above the round-1 floor
(`#0x1000`) and below `__kernel_text_start`, so the raise is the only variable between the two boots.

| file | tree | what it shows |
|---|---|---|
| `mutation-2-asm-bound-lowered-legF-red.txt` | `972a0832` + the assembly bound lowered back to `cmp x1, #0x1000` | at `:183`, immediately after the substitution: `[FATAL_REGS] label=INSTRUCTION_ABORT cpu=3 spsr=0x20000305 esr=0x8600000d far=0x1000000 elr=0x1000000`, `[FATAL_POSTMORTEM] cpu=3` at `:200`, and leg F `fatal=1 … FAIL` at `:2892` |
| `legF-green-floor-contained.txt` | `972a0832` | no abort at `0x1000000` anywhere, leg F `fatal=0 … PASS` at `:490`, `[BOOT_TESTS:PASS]` — **and see the disclosure below** |

Leg F's verdict reads `any_fatal_postmortem_captured()` as well as `armed`/`fired`, because the
kernel survives the substituted dispatch in both configurations — a liveness-only predicate printed
`PASS` on both sides and could not discriminate. The abort record and the postmortem flag are what
separate them. The `fatal` term only covers what happened *before* the leg reports, which is
sufficient here: the red boot's abort lands at `:183` and the report at `:2892`.

In the red serial the `[INSTRUCTION_ABORT] FAR=… ELR=… ESR=…` header line is torn by interleaved
output and only the `[FATAL_REGS]` record survives intact. The service-sequence gate's
`instruction_abort_signatures()` unions both record forms, so a boot in this state still classifies
field-exactly; the same thing happened to leg K's round-1 red serial.

## Disclosure — #635's abort family was observed once on the fixed tree

`legF-green-floor-contained.txt:716`, on `972a0832` with both round-2 nets in place, after that
boot's `[BOOT_TESTS:PASS]`:

```
[INSTRUCTION_ABORT] FAR=0xffff000054276f28 ELR=0xffff000054276f28 ESR=0x8600000e IFSC=0xe TTBR0=0x100004406c000 from_el0=0
[FATAL_REGS] label=INSTRUCTION_ABORT cpu=0 spsr=0x20000305 ... sp=0xffff000054276ef0
  x16=0x0 x17=0x4b3 ... x25=0x4b3 ... x28=0x0 x29=0x1 x30=0xffff000054276f28
```

Same ESR/IFSC and the same `FAR == ELR == x30 ==` a kernel stack address as #635. **The round-2
containment therefore must not be read as closing #635**, and this PR does not claim it does.

The register file says why, and it is worth recording: here `x29 = 0x1` — *not* equal to `x30`, unlike
#635's field captures where `x29 == x30` and `x19`/`x26`/`x29`/`x30`/`x20`/`x27`/`x21` held six
*consecutive* stack slots. And `x30` is `sp + 0x38`, a slot inside the faulting frame itself. That is
the shape of an ordinary compiled epilogue — `ldp x29, x30, [sp, #0x38]` — reloading a saved-LR slot
that something had overwritten, and then `ret`. No dispatch helper is involved in such a transfer and
no resume-PC predicate can see it, which is consistent with the fix being at the consumer while the
producer stays open on #635. `x17 == x25 == 0x4b3 == 1203`, a live tid, is the #633 tid-as-value
signature appearing in the same register file.

What round 2 does prove about this consumer is mutation 1: with the text-range bound removed, the
ret-based dispatch *does* transfer to a stack PC and produce this exact field set; with it in place it
refuses and names the thread. Whether the ret dispatch is also a producer of the field occurrences is
not settled by either serial.

## Control

`control-r2-disarmed-972a0832.txt` — `972a0832` built with `--features boot_tests` and **neither**
new feature: 0 `[INSTRUCTION_ABORT]`, 0 `[RET_DISPATCH_REFUSED:`, 0 `[TEST:…:FAIL:…]`,
`[BOOT_TESTS:PASS]`. The two new stimuli manufacture no unrelated collateral.

## What the two round-2 mutations prove

| mutation | net removed | result | therefore |
|---|---|---|---|
| 1 | the Rust text-range bound (reverted to round 1's address-space bound) | leg T `refused=0:FAIL` **and** #635's field set | the text-range narrowing is what closes the garbage-kernel-PC face; the assembly floor structurally cannot, because a stack address is above kernel text |
| 2 | the raised assembly floor (lowered back to `#0x1000`) | leg F `fatal=1:FAIL` **and** `[INSTRUCTION_ABORT] ELR=0x1000000` | the raise is load-bearing for values between `0x1000` and kernel text — the `<tid>`-as-PC range of #633 among them — in the window the Rust predicate does not cover |

Round 1's mutations A and B still stand for leg K and are unchanged by this round.

## Alias finding

`rust-objdump` on the built `kernel-aarch64` shows the inlined predicate materialising its bounds
PC-relatively — `adr x10, 0xffff000040400000` for `__kernel_text_start` (the linker relaxed the
`ADRP+ADD`) and `adrp x9 / add x9, x9, #0x0` for `__kernel_text_end` — so the window follows whichever
alias is executing: the high-half HHDM alias on QEMU, the identity-mapped physical alias on Parallels,
where the loader enters `kernel_main` at a physical address. The counterpart alias is admitted through
a second window anyway, so a PC spelled the other way is not refused for its spelling. The live text
window on this build is `[0xffff000040400000, 0xffff000040600000)`; #635's `0xffff000054243f00` is
outside both windows.

## Round-2 service-sequence gate — the two reds it produced

`run-aarch64-service-sequence-gate.sh --boots 25 --profile both` on `71cceff0`: **48/50 GREEN, 2
`UNATTRIBUTED`, gate FAILED**. Full log: `gate-r2-ss25-clean-run.log`.

| file | profile / boot | record | attribution |
|---|---|---|---|
| `gate-r2-ss25-max-boot11-pcalign-elr5-633.txt:684` | max, 11 | `[PC_ALIGN] ELR=0x5 FAR=0x5 from_el0=0 cpu=1`, `x30=0x5`, `x29=0x1f`, `spsr=0x20000005`, `DISPATCH_TRACE[0] U old=5` | **#633**. `0x5` is thread id 5, which is that CPU's *outgoing* tid at the preceding dispatch — the rule the `0x4b1`/`0x4b5`/`0x4b7` captures also fit, made obvious by a tid too small to be anything else. |
| `gate-r2-ss25-cortexa72-boot7-el0-kernel-pc-8200000e.txt:632` | cortex-a72, 7 | `[INSTRUCTION_ABORT] FAR=0xffff000040800000 ELR=0xffff000040800000 ESR=0x8200000e IFSC=0xe from_el0=1`, victim SIGSEGVs | **#637**, filed for it. EC=0x20 is an abort from a *lower* EL: a userspace thread resumed at a kernel address, past `__kernel_text_end`. A second value shape in the "EL0 resume PCs are unvalidated" gap. |

Neither is reachable by this round's predicates, and the run proves it rather than asserting it:
across all 50 boots there are **zero** `[RET_DISPATCH_REFUSED:` lines and **zero**
`WARN: bad elr … redirecting to idle` lines, so neither predicate returned false once. `0x5` is below
both versions of the assembly floor, so that transfer never went through
`aarch64_ret_to_kernel_context`; `from_el0=1` puts the other on a path this round does not touch.

An earlier run of the same gate was discarded at boot 9 rather than reported: starting the
production-profile boot test (which builds *without* `boot_tests`) during it hardlinked a different
kernel onto `target/aarch64-breenix-kernel/release/kernel-aarch64`, the landmine
`run-aarch64-service-sequence-gate.sh:146-153` documents. The kernel was rebuilt with `boot_tests`
and the gate re-run with nothing else touching the tree.
