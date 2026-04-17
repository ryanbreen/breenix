# F21 Phase 3: Diff Analysis

## Corrected Regressing Commit

```text
f97402247d63c94212061479964f2df65ecfa2ad
feat: cross-platform SMP — forward CPU 0's MMU config to secondary CPUs
```

The initial exploratory bisect result, `bfdb60b8`, was rejected because that
commit intentionally rendered a full-screen red VirGL quad. A later commit,
`9b56273b`, captured non-red rendered content, proving the display pipeline
recovered after that intentional red test. The corrected bisect therefore used
`9b56273b` as the good bound and treated rendered non-red content as good.

## What Changed

`f9740224` changed ARM64 SMP bring-up, not the VirGL command stream:

- `kernel/src/arch_impl/aarch64/boot.S`
  - Secondary CPUs stopped loading hardcoded `MAIR_EL1`, `TCR_EL1`, `ttbr0_l0`,
    and `ttbr1_l0`.
  - Secondary CPUs began reading `SMP_MAIR_PHYS`, `SMP_TCR_PHYS`,
    `SMP_TTBR0_PHYS`, and `SMP_TTBR1_PHYS`, which CPU 0 publishes from its
    live registers.
- `kernel/src/arch_impl/aarch64/smp.rs`
  - `flush_boot_page_tables()` was replaced by `set_smp_ttbrs()`.
  - CPU 0 now writes TTBR0/TTBR1/MAIR/TCR values into `.bss.boot` variables for
    secondary CPUs.
- `kernel/src/main_aarch64.rs`
  - Boot now calls `set_smp_ttbrs()` before probing secondary CPUs.

## Evidence

Corrected bisect:

```text
good 9b56273b  -> rendered non-red, dominant (30,30,40)
good e6a4f61d  -> rendered non-red, bwm starts and composites
bad  f9740224  -> solid red, first bad
bad  28f6762b  -> solid red
bad  current   -> solid red
```

Good parent `e6a4f61d` still probes secondary CPUs on Parallels, but the boot
continues after they fail to come online:

```text
[smp] Timeout waiting for CPUs (1  online, 4 expected)
[smp] 1 CPUs online
Breenix ARM64 Boot Complete!
[init] Starting /bin/bwm...
[bwm] GPU compositing mode (VirGL), display: 1280x960
[gpu-perf] frame=500 ...
```

First bad `f9740224` initializes VirGL successfully, then crashes during or just
after SMP probing:

```text
[virgl] Step 10: SET_SCANOUT + RESOURCE_FLUSH
[virgl] VirGL 3D pipeline initialized successfully
[smp] Probing secondary CPUs via PSCI...
[DATA_ABORT] FAR=0xffff00000200000c ELR=0xffff000040144678 ...
```

The Parallels run log also records VCPU exceptions immediately after SMP startup:

```text
VCPU1 return code 2: exception generation at ffff00004017acd0
```

## Theory

The current solid-red capture is not caused by `RESOURCE_FLUSH` failing to
present. The VirGL initialization path reaches `SUBMIT_3D OK`,
`SET_SCANOUT`, and `RESOURCE_FLUSH` before the failure. The visible red remains
because `f9740224` makes Parallels secondary CPU bring-up fault before userspace
and bwm can render a non-red compositor frame.

In other words, the scanout symptom is downstream of a Parallels SMP boot
regression. The minimal fix should avoid Parallels PSCI secondary CPU bring-up
until the secondary CPU MMU/stack path is made robust, while preserving SMP on
platforms where it is currently expected to work.

Post-fix validation exposed one additional issue in the kernel VirGL init path:
the proof `DRAW_VBO` batch was still drawing a full-screen red quad. Red is the
F21 failure sentinel, so a successful scanout with no later compositor frame was
indistinguishable from a failed scanout. That red draw came from
`kernel/src/drivers/virtio/gpu_pci.rs`, not from the bisected SMP commit.

## Minimal Fix Direction

Gate ARM64 PSCI secondary CPU bring-up to QEMU and VMware for now. On Parallels,
log a skip and continue single-CPU boot so the already-initialized VirGL display
can reach userspace and bwm can composite.

Also restore the kernel VirGL baseline to the documented known-good color:
cornflower blue for both the initial `CLEAR` and the full-pipeline proof
`DRAW_VBO`. This preserves the shader/VBO exercise while making the displayed
baseline non-red and comparable to the March 2026 known-good capture.
