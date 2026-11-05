# Breenix window manager

## Closing a window

The first close-button request sends the window a `CloseRequested` event. A second
request sends SIGTERM to its owner and starts a 2,000 ms UI-clock deadline. If the
window remains in LIST at that deadline, bwm sends SIGKILL to the owner. Further
requests do not postpone the deadline. When the window leaves LIST, bwm forgets
its pending close state.

## Termination custody

bwm blocks SIGTERM before startup GPU work. Its handler only sets the
termination flag. The input/scene poll boundary unblocks TERM, skips the poll
if the flag is already set, and blocks TERM again before inspecting the flag.
A requested shutdown releases the compositor claim, returns the display, and
exits 0. A failure returning custody is reported and exits 1.

Hotkey launches use direct spawn while bwm keeps TERM blocked. Direct spawn
creates fresh signal state rather than inheriting bwm's blocked mask and
handler; the child starts with TERM unblocked and at default disposition.
The recording host check in `tests/bwm_shutdown.rs` executes the hotkey and
spawn bodies for both architectures with the production signal state. It
checks delivery to the child while the parent's pending TERM stays blocked.
The suite's SIGKILL cleanup path is unchanged. ARM still needs a booted check
of this shutdown and launch behavior when ARM execution is released.

### Launch errors

A failed launch opens one bwm-owned BreenGL window. It identifies a missing executable, a fork failure, or another spawn error. A launch that has not presented a window within five seconds of UI time reports a timeout. Later errors replace its contents; closing the window dismisses it. Suite-mode point 34 can refuse one fork for a target caller.
