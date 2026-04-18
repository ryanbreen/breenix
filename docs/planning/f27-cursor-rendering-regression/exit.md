# F27 Exit - Mouse Cursor Rendering Regression

Date: 2026-04-18

Branch: `f27-cursor-rendering-regression`

Base: `ab351efe`

PR: https://github.com/ryanbreen/breenix/pull/322

## What I Built

- `docs/planning/f27-cursor-rendering-regression/phase1.md`: recorded the layer-localization evidence and regression range.
- `userspace/programs/src/init.rs`: fixed the x86_64 build warning by making the `yield_now` import aarch64-only.
- `userspace/programs/src/bwm.rs`: added an aarch64-only software cursor overlay for the direct VirGL blit path, with arrow and resize shapes tracked from bwm's existing cursor-shape logic.
- `docs/planning/f27-cursor-rendering-regression/phase3.md`: recorded build, capture, cursor pixel-mask, and movement-probe validation.
- `docs/planning/f27-cursor-rendering-regression/exit.md`: this exit report.

## Original Ask

Determine where the F27 cursor regression occurred, bisect likely F22/F25/F26
GUI changes, restore cursor rendering without reverting F1-F26 or regressing F26
animation performance, validate with Parallels capture evidence, then open and
merge a PR.

## How This Meets The Ask

Phase 1 layer localization: implemented. `phase1.md` records that F22 moved the
aarch64 bwm path to direct `virgl_composite()`, while the kernel cursor quad is
only drawn by the op16 `virgl_composite_windows*()` path.

Phase 2 bisect/regression range: implemented by history inspection. The likely
regression point is F22 (`657d18c0`), with F24/F26 preserving that direct path.
No F1-F26 revert was used.

Phase 3 fix: implemented. `bwm.rs` now draws the cursor into the aarch64
direct-blit desktop buffer immediately before `graphics::virgl_composite()`.
Mouse input remains event-driven through `compositor_wait`; no polling was
added.

Phase 4 validation: partial/implemented. A one-off `f27cursor-*` Parallels VM
booted this branch, rendered bwm+bounce, passed strict F23 render verdict, and
the capture pixel-mask detector found an arrow-shaped cursor region. A
best-effort host-pointer movement probe rendered the cursor at a non-default
kernel-reported location, but `prlctl` has no mouse injection API and the
synthetic Quartz move did not produce a second in-boot delta.

## What I Did Not Build

- I did not restore the aarch64 op16 VirGL multi-window compositor path; that was
  intentionally avoided to preserve the F26 direct-blit FPS path.
- I did not add polling.
- I did not modify Tier 1 prohibited files.
- I did not commit screenshots, serial logs, or generated disk artifacts.

## Known Risks And Gaps

- Cursor rendering on aarch64 is now duplicated in bwm rather than sharing the
  kernel VirGL cursor atlas. That is deliberate because the aarch64 path does
  not use the kernel cursor-quad compositor.
- The in-boot HID movement proof is partial because Parallels CLI supports
  keyboard event injection only. The code path updates cursor coordinates from
  existing bwm mouse-event handling, and the movement probe did show a
  non-default cursor position.
- `cargo run -p xtask -- boot-stages` was attempted during Phase 1 and failed at
  a pre-existing x86_64 `sigaltstack()` marker after reaching 162/252 stages.
  The final gate therefore uses clean builds plus Parallels capture validation.

## How To Verify

```bash
./userspace/programs/build.sh --arch aarch64
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
bash scripts/f23-render-verdict.sh /tmp/f27cursor-1776510485-screen.png
grep -E "SOFT_LOCKUP|SOFT LOCKUP|TIMEOUT|UNHANDLED|DATA_ABORT|FATAL|panic|PANIC" \
  /tmp/f27cursor-1776510485-serial.log
```

For the final run, `grep -E '^(warning|error)'` over all three build logs
produced no output. The fault-marker grep also produced no output.

## Self-Audit

- No polling added.
- No Tier 1 prohibited files modified.
- F1-F26 intact; no revert used.
- F26 direct `virgl_composite()` frame path preserved.
- One-off `f27cursor-*` validation VMs were stopped and deleted.
