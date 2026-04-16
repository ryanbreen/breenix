# Decisions - F12 SGI Linux Parity

## 2026-04-16T06:08:00-04:00 - Use a clean F12 worktree
**Choice:** Created `/Users/wrb/fun/code/breenix-worktrees/f12-sgi-linux-parity` from `diagnostic/f11-send-sgi-boundary`.
**Alternatives considered:** Reusing `/Users/wrb/fun/code/breenix` or the F11 worktree.
**Evidence:** The main checkout is on `diagnostic/f6-gic-stuck-state` with unrelated dirty files. The F11 worktree also has uncommitted F11 docs/artifacts.

## 2026-04-16T06:10:00-04:00 - Match Linux SGI ordering at the call boundary
**Choice:** Add the Linux pre-IPI `dsb ishst` inside Breenix `send_sgi()` before writing `ICC_SGI1R_EL1`, while keeping the existing post-write `isb`.
**Alternatives considered:** Moving the barrier to callers.
**Evidence:** Breenix has a single `send_sgi()` helper for IPI emission; placing the barrier there makes every SGI call match Linux's `gic_ipi_send_mask()` ordering without touching scheduler wake semantics.
