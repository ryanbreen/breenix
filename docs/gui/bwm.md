# Breenix window manager

## Closing a window

The first close-button request sends the window a `CloseRequested` event. A second
request sends SIGTERM to its owner and starts a 2,000 ms UI-clock deadline. If the
window remains in LIST at that deadline, bwm sends SIGKILL to the owner. Further
requests do not postpone the deadline. When the window leaves LIST, bwm forgets
its pending close state.
