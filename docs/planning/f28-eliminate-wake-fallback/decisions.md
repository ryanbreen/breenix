# Decisions - F28 Eliminate the 5ms Wake Fallback

## 2026-04-18T10:53:11Z - Instrument Before Fixing

**Choice:** Commit counter instrumentation before changing wake ordering.
**Alternatives considered:** Apply the suspected compositor wait race fix immediately.
**Evidence:** The factory contract requires Phase 2 reproduction numbers from the instrumented pre-fix path.

## 2026-04-18T11:05:00Z - Fix Compositor Waiter Publication Ordering

**Choice:** Publish `COMPOSITOR_WAITING_THREAD` only after `block_current_for_compositor()` marks bwm blocked, then immediately re-check all readiness signals before WFI.
**Alternatives considered:** Only re-check `COMPOSITOR_DIRTY_WAKE` after the existing waiter publish, or add a longer fallback.
**Evidence:** Phase 2 measured `event=0 fallback=2191`, proving the event-driven path was not delivering client wakes. Publishing a waiter before the thread is blocked creates the same lost-wake class F26 fixed on the client side.

