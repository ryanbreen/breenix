# Preserved serials — #596 (aarch64 EL1 DATA_ABORT in `schedule_from_kernel`, FAR=0x8)

All four were produced on this host with
`docker/qemu/run-aarch64-service-sequence-gate.sh --profile cortex-a72`, against kernels built
from `aarch64-breenix-kernel.json` (soft-float) with `--features boot_tests` (plus
`force_eret_dispatch_596` where noted). Each one is a **red** run kept as filed evidence.

| file | kernel | what it proves |
|---|---|---|
| `oracle-red-prefix-plain-cortexa72-boot1.txt` | `main` + the oracle commit only (no fix), `boot_tests` | The runtime oracle is RED on unmodified `main` in the plain gate, **without** the forcing knob: `[CTX596_ORACLE:FAIL:save_elr_mismatch … ctx_elr=0xffff0000405630a4 x30=0xffff0000405640c4]`. 6/6 boots red in 2–4 s. The inline save's recorded resume PC is not its `x30`. |
| `oracle-red-prefix-forced-cortexa72-boot1.txt` | same, `force_eret_dispatch_596` | The dangerous consumer is real: `[CTX596_ORACLE:FAIL:eret_resume_pc:tid=185:cpu=2:frame_elr=0xffff000040452fbc:x30=0xffff00004045406c]` — an inline-saved context **actually ERET-dispatched at the wrong PC**. 4/4 boots. |
| `mutation-m1-oracle-red-cortexa72-boot1.txt` | fix branch with **only** the assembly `str x30, [x0, #264]` deleted | The designated mutation that reintroduces the defect shape trips the oracle: 4/4 boots red. |
| `mutation-m3-dataabort-far8-cortexa72-boot3.txt` | fix branch with the assembly store deleted **and** the inline-aware ERET resume selection reverted, `force_eret_dispatch_596` | **The field-exact #596 fault, reproduced.** `[DATA_ABORT] FAR=0x8 … ESR=0x96000005 DFSC=0x5 from_el0=0`, `x19=0x0`, `x20=0xffff000040800008` (`&SCHEDULER+8`), `x22=0xffff000040800000` (`&SCHEDULER`), `x25=0x0`, `sp=0xffff000054262e80` — the same register fingerprint as the filed report. It also demonstrates the rider fix (#597): the postmortem is **readable** (`[FATAL_REGS]`, `DISPATCH_TRACE`, `[FATAL_POSTMORTEM]`, trace buffers) where every previously filed #596 serial truncated mid-line. |

The corresponding green evidence is not preserved here: with the fix in place the forced-ERET
build runs the same gate with bucket `596 = 0` while the `[CTX596_ELR_DIVERGENCE]` marker still
fires on 6/6 boots (48 lines) — the mechanism is live, and the repair neutralises it.
