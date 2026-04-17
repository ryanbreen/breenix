# F22 Exit: bwm Userspace GUI Rendering on Single-CPU Parallels

## What I built

- `logs/f22-phase1/findings.md` - Phase 1 lifecycle trace. It records the last emitted checkpoint from the initial boot and identifies bwm startup / first compositor presentation as the next failure area.
- `userspace/programs/src/bsh.rs` - changed JavaScript `spawn()` to use the kernel `spawn` syscall directly instead of fork-plus-exec, then yield once after successful child creation.
- `userspace/programs/src/init.rs` - changed init service launch to use kernel `spawn`; on aarch64, init now starts the minimal early-visible service set (`telnetd`, `bwm`) directly and emits `[init] Boot script completed` before starting bsshd.
- `userspace/programs/src/bwm.rs` - made the aarch64 early boot compositor avoid filesystem-backed font/hotkey loads, avoid extra full-screen background/shadow allocations, and present the CPU-composited desktop through the direct VirGL blit path.
- `kernel/src/syscall/graphics.rs` - routes aarch64 VirGL op10 composite calls through `virgl_composite_frame`, the path already proven to take over scanout on Parallels.
- `kernel/src/task/completion.rs` and `kernel/src/task/scheduler.rs` - gated ARM64-only AHCI wake tracing helpers so x86_64 builds remain warning-free.
- `scripts/create_ext2_disk.sh` - removed boot-script sleeps and re-ordered generated `/etc/init.js` so the compositor launches before lower-priority background services on platforms that still run bsh/init.js.
- `.factory-runs/f22-bwm-userspace-gui/exit.md` - runbook-local copy of this exit report.

## What The Original Ask Was

F22 needed to trace why the F21 kernel VirGL proof draw remained visible on single-CPU Parallels, fix the first missing bwm lifecycle checkpoint, capture the display, and merge a PR only if the capture showed non-blue, non-monochrome GUI content.

## How This Meets The Ask

- **Phase 1 lifecycle trace: implemented.** Commit `e7b20edd` records `logs/f22-phase1/findings.md`. The initial trace showed the kernel VirGL proof draw completed, init started, but `[init] Boot script completed` was absent and the userspace GUI stack did not take over scanout.
- **bwm process launch: implemented for the visible desktop path.** `userspace/programs/src/init.rs:36` starts the aarch64 early services directly with `spawn`, including `/bin/bwm` at `userspace/programs/src/init.rs:42`.
- **bsh spawn path: implemented.** Platforms that still run `/etc/init.js` now have `spawn()` backed by the kernel spawn syscall at `userspace/programs/src/bsh.rs:1286`.
- **bwm early compositor presentation: implemented.** bwm avoids early aarch64 filesystem reads at `userspace/programs/src/bwm.rs:1399`, uses built-in hotkeys at `userspace/programs/src/bwm.rs:1463`, and submits the first desktop frame with `graphics::virgl_composite` at `userspace/programs/src/bwm.rs:1474`.
- **SET_SCANOUT takeover from userspace: implemented for ARM64 direct blit.** The aarch64 VirGL composite syscall now calls `virgl_composite_frame` at `kernel/src/syscall/graphics.rs:660`, which performs the transfer/flush/scanout path that replaced the blue proof draw in validation.
- **Phase 3 capture: implemented.** Final capture path:

```text
logs/f22-validation/capture-f22-final.png
logs/f22-validation/capture-f22-final.png.stats.json
```

Stats:

```text
dominant_rgb=[12,16,39]
distinct_colors=102
solid_red=false
passes_rendered_desktop_bar=true
verdict=GOOD
```

- **Phase 4 merge: pending in this document at first commit.** PR URL will be added before merge.

## What I Did Not Build

- Full GUI client restoration is not complete. The final aarch64 early service list intentionally starts only `telnetd` and `bwm` before bsshd; `bterm`, `blog`, `bounce`, and `bcheck` remain follow-up work.
- The op16 `virgl_composite_windows_rect` SUBMIT_3D window-compositor path still times out on aarch64 Parallels. This fix uses the op10 direct blit path for aarch64 bwm presentation.
- aarch64 bwm early boot does not load `/etc/fonts.conf` or `/etc/hotkeys.conf`; it uses bitmap font fallback and built-in hotkeys until the AHCI/userspace load ordering issue is resolved.
- The final serial log reaches `[spawn] path='/bin/bsshd'`, but does not show `[init] bsshd started` before the successful capture window.

## Known Risks And Gaps

- The direct-blit path proves userspace can take over scanout and render the desktop bar, but it is not the final per-window VirGL 3D compositor.
- Early aarch64 userspace still appears sensitive to large AHCI-backed ELF/config reads during the single-CPU boot window.
- Because aarch64 init bypasses bsh/init.js for the early visible path, future work should restore script-driven launch after the underlying spawn/load stall is fixed.

## How To Verify

Clean x86_64 build:

```bash
bash -lc 'set -o pipefail; cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | tee /tmp/f22-x86-build.log; build_rc=${PIPESTATUS[0]}; test $build_rc -eq 0 && ! grep -E "^[[:space:]]*(warning|error)(\\[|:)" /tmp/f22-x86-build.log'
```

Clean aarch64 kernel build:

```bash
bash -lc 'set -o pipefail; cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 --features testing,external_test_bins 2>&1 | tee /tmp/f22-kernel-aarch64-build-final.log; build_rc=${PIPESTATUS[0]}; test $build_rc -eq 0 && ! grep -E "^[[:space:]]*(warning|error)(\\[|:)" /tmp/f22-kernel-aarch64-build-final.log'
```

Clean aarch64 userspace build:

```bash
bash -lc 'set -o pipefail; userspace/programs/build.sh --arch aarch64 2>&1 | tee /tmp/f22-userspace-aarch64-build.log; build_rc=${PIPESTATUS[0]}; test $build_rc -eq 0 && ! grep -E "^[[:space:]]*(warning|error)(\\[|:)" /tmp/f22-userspace-aarch64-build.log'
```

Display capture verdict:

```bash
F21_RUN_DIR=/Users/wrb/fun/code/breenix/logs/f22-validation \
F21_CAPTURE_OUT=/Users/wrb/fun/code/breenix/logs/f22-validation/capture-f22-final.png \
F21_SCRATCHPAD=/Users/wrb/fun/code/breenix/.factory-runs/f22-bwm-userspace-gui/scratchpad.md \
scripts/f21-bisect-verdict.sh
```

Expected result:

```text
GOOD
passes_rendered_desktop_bar=true
dominant=12,16,39
distinct=102
```

## PR

https://github.com/ryanbreen/breenix/pull/315

## Self-Audit

- No prohibited Tier 1 files were modified.
- No polling loops were added.
- F1-F21 / PR #314 were not reverted.
- The final validation capture is not cornflower blue dominant and is not monochrome.
- QEMU/Parallels cleanup is required before handoff.
