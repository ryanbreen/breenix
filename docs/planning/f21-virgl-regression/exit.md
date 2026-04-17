# F21 Exit: Kernel VirGL Scanout Regression Bisect

## Outcome

F21 found that the current solid-red Parallels capture was caused by two kernel-side issues:

1. The first bisectable regression is `f97402247d63c94212061479964f2df65ecfa2ad`
   (`feat: cross-platform SMP - forward CPU 0's MMU config to secondary CPUs`).
   On Parallels, this commit starts secondary CPUs through PSCI and then faults
   before userspace can launch bwm, leaving the early GPU content visible.
2. The early VirGL proof draw still rendered a full-screen red quad. Red is the
   F21 failure sentinel, so a successful scanout with no later compositor frame
   looked exactly like a scanout failure.

The fix keeps Parallels on the single-CPU boot path until the secondary CPU
MMU/stack path is fixed, and changes the kernel VirGL init proof draw from red
to the documented cornflower-blue baseline.

## Known-Good SHA

```text
e47c96b24b861a4f69f32a61630651fe312109b9
feat: VirGL 3D rendering visible on Parallels display - cornflower blue!
```

Capture:

```text
logs/f21-virgl-regression/known-good-e47c96b2.png
dominant_rgb=[100,149,237]
distinct_colors=1
solid_red=false
```

## Corrected Bisect SHA

```text
f97402247d63c94212061479964f2df65ecfa2ad
feat: cross-platform SMP - forward CPU 0's MMU config to secondary CPUs
```

The initial exploratory bisect result, `bfdb60b8`, was rejected because that
commit intentionally drew a full-screen red VirGL quad and later commit
`9b56273b` captured rendered non-red output.

## Diff Analysis Summary

`f9740224` did not change the VirGL command stream. It changed ARM64 SMP
bring-up so secondary CPUs consume CPU 0's live MMU configuration. On Parallels,
boot reaches:

```text
[virgl] Step 10: SET_SCANOUT + RESOURCE_FLUSH
[virgl] VirGL 3D pipeline initialized successfully
[smp] Probing secondary CPUs via PSCI...
```

and then faults with data aborts / VCPU exceptions before userspace starts. The
good parent `e6a4f61d` times out secondary CPU probing, continues boot, and bwm
renders non-red content.

## Fix Description

- `kernel/src/main_aarch64.rs`
  - Gate ARM64 PSCI secondary CPU bring-up to QEMU and VMware.
  - Skip Parallels secondary CPU startup and continue single-CPU boot.
- `kernel/src/drivers/virtio/gpu_pci.rs`
  - Use cornflower blue for the initial VirGL `CLEAR`.
  - Use cornflower blue for the full-pipeline proof draw constant buffer, keeping
    the shader and VBO exercise but removing red as initial displayed content.
- `scripts/f21-bisect-verdict.sh`
  - Added the repeatable F21 build/boot/capture verdict script.
- `scripts/parallels/capture-display.sh`
  - Landed the F20f capture tool on this branch for bisect and validation.

## Post-Fix Validation

```text
F21_CAPTURE_SCRIPT=/Users/wrb/fun/code/breenix/scripts/parallels/capture-display.sh \
F21_RUN_DIR=/tmp/f21-postfix2 \
F21_CAPTURE_OUT=/tmp/f21-postfix2/postfix-capture.png \
F21_SCRATCHPAD=/Users/wrb/fun/code/breenix/.factory-runs/f21-virgl-regression-20260417-145226/scratchpad.md \
bash scripts/f21-bisect-verdict.sh
```

Result:

```text
GOOD
dominant_rgb=[100,149,237]
distinct_colors=1
redish_fraction=0.0
solid_red=false
passes_rendered_desktop_bar=false
```

Captured PNG:

```text
logs/f21-virgl-regression/postfix-capture.png
logs/f21-virgl-regression/postfix-capture.png.stats.json
```

## PR

https://github.com/ryanbreen/breenix/pull/314

## Self-Audit

- No polling or busy-wait code added.
- No prohibited Tier 1 files modified.
- F1-F20e required PRs were not reverted.
- Parallels remains single-CPU until the secondary CPU MMU/stack issue is fixed.
- QEMU process cleanup was run after Parallels/QEMU verification.
