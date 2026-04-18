# F27 Phase 1 - Cursor Regression Layer Localization

Date: 2026-04-18

## Summary

The cursor regression is in the aarch64 bwm presentation layer, not in cursor
texture initialization or cursor-shape plumbing.

F22 moved the aarch64 bwm render path to `graphics::virgl_composite()` because
the op16 multi-window VirGL compositor path timed out on Parallels after bwm
took over scanout. That direct-blit path uploads and displays the CPU-composited
desktop, but it does not invoke the kernel's GPU cursor-quad renderer.

The cursor renderer still exists in `kernel/src/drivers/virtio/gpu_pci.rs` and
is only reached from `graphics::virgl_composite_windows*()` / op16. The current
aarch64 bwm loop never calls that path, so mouse movement can update bwm state
without producing a visible cursor glyph.

## Evidence

- `userspace/programs/src/bwm.rs` uses `graphics::virgl_composite()` for the
  aarch64 initial composite and every aarch64 frame.
- The non-aarch64 path still calls `graphics::virgl_composite_windows_rect()`,
  whose kernel implementation explicitly handles cursor-only redraws.
- `kernel/src/syscall/graphics.rs` documents that VirGL cursor-only frames fall
  through to `virgl_composite_windows()`.
- `kernel/src/drivers/virtio/gpu_pci.rs` initializes the cursor atlas as
  resource id 6 and draws it in `virgl_composite_single_quad()`, which is only
  called by `virgl_composite_windows()`.
- bwm still processes `COMPOSITOR_READY_MOUSE`, calls `mouse_state_with_scroll()`,
  routes mouse events, and calls `set_cursor_shape()`. Those calls update state
  but do not draw anything on the aarch64 direct-blit path.

## Regression Range

The likely regression point is F22 (`657d18c0 fix(gui): F22 render bwm desktop
on Parallels`), which introduced the aarch64 direct `virgl_composite()` fallback.
F24 then added mapped-window blitting on top of that direct path. F26 preserved
the same bwm presentation path while improving frame pacing, so the fix should
not revert F26.

## Validation Notes

Initial `cargo run -p xtask -- boot-stages` on x86_64 reached QEMU but surfaced
an unrelated compile warning first: `userspace/programs/src/init.rs` imported
`yield_now` in an x86_64 build even though its uses are aarch64-only. The Phase
1 commit fixes that conditional import so later quality gates can be clean.

The same run ended at `162/252` stages with the first missing marker reported as
`sigaltstack() syscall verified`. The user output showed the sigaltstack test
had started and passed its first checks, so this is recorded as a pre-existing
x86 boot-stage validation failure for this branch rather than cursor evidence.
