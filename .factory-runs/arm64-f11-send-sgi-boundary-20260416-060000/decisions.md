# Decisions - F11 Send SGI Boundary

## 2026-04-16T06:00:00-04:00 - Reuse AHCI ring for SGI breadcrumbs

**Choice:** Add SGI and wake-buffer site tags to the existing AHCI diagnostic
ring and encode `target_cpu` in `slot_mask`.

**Alternatives considered:** Add a new trace structure or log messages inside
SGI delivery.

**Evidence:** F10 already used the AHCI ring from ISR context and encoded SGI
target CPU in `slot_mask`. The F11 contract forbids semantic changes, locks,
allocations, logging, and new trace-ring fields inside `send_sgi()`.
