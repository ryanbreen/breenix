# F28 Phase 1 - Wake Counter Instrumentation

Date: 2026-04-18

## What Changed

`kernel/src/syscall/graphics.rs` now tracks client frame wait outcomes:

- `FRAME_WAKE_EVENT_COUNT`: incremented when `handle_composite_windows()` explicitly unblocks a client waiting for compositor upload completion.
- `FRAME_WAKE_FALLBACK_COUNT`: incremented when `mark_window_dirty` resumes and the window still contains the same waiting thread ID, proving the 5 ms timeout woke the client before compositor consumption did.
- `maybe_dump_frame_wake_counts()`: emits a compact `[gfx-wake] event=... fallback=... total=... fallback_ppm=...` line at most once every five seconds from `compositor_wait`.
- `maybe_dump_frame_wake_counts_by_total()`: also emits the same compact line every 1000 counted client frame waits, so post-fix validation still records counters when the event-driven path is fast.

The instrumentation does not touch Tier 1 prohibited files and does not add per-frame serial logging.

## Validation

Command:

```bash
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f28-m1-kernel-build.log
grep -E '^(warning|error)' /tmp/f28-m1-kernel-build.log
```

Result: build exited 0, and the warning/error grep produced no output.

Follow-up hardening after the first post-fix validation: the time-based dump alone did not emit under the fixed wake path, so total-count bucket dumps were added. The follow-up aarch64 kernel build also exited 0 with no warning/error lines.

## Notes

Workspace `cargo fmt --check` still fails on pre-existing unrelated formatting/trailing-whitespace issues outside this change. `kernel/src/syscall/graphics.rs` was formatted directly with `rustfmt`.
