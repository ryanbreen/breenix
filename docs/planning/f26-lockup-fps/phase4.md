# F26 Phase 4 - Bounce FPS

Date: 2026-04-18

## Before

Phase 1 measured the F25 per-frame sleep build at about 70 Hz:

```text
first measured frame: Frame #500 near ticks=5000
last measured frame:  Frame #11000 near ticks=155000
elapsed frames:       10500
elapsed time:         150 seconds
estimated FPS:        70 Hz
```

## Failed Experiments

Removing the sleep without removing font I/O reproduced AHCI timeouts after bounce started.

Disabling font polling after `Window::new()` was too late; the initial font load inside `Window::new()` could still wedge before `[bounce] Window mode`.

Creating the window without fonts but running a fully tight present loop avoided AHCI timeouts but froze after early bwm discovery.

Sleeping every fourth frame stayed stable but measured only 25 Hz, because missed compositor wakes still fell back to the 50 ms client timeout.

## After

With no bounce font I/O and a 5 ms compositor back-pressure fallback, the final validation measured about 160 Hz:

```text
first measured frame: Frame #1000 near ticks=5000
last measured frame:  Frame #17000 near ticks=105000
elapsed frames:       16000
elapsed time:         100 seconds
estimated FPS:        160 Hz
```

This clears the Phase 5 threshold of at least 100 Hz.
