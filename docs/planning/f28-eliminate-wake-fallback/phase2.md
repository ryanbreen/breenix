# F28 Phase 2 - Baseline Wake Ratio

Date: 2026-04-18

## Workload

Command:

```bash
./run.sh --parallels --test 120
```

Serial artifact:

```text
logs/f28/baseline/serial.log
```

The run completed its host-side 120 second wait. The Parallels screenshot helper failed to find the VM window, so Phase 2 uses serial evidence only. Fault-marker grep was empty.

## Counter Result

Final wake counter line:

```text
[gfx-wake] event=0 fallback=2191 total=2191 fallback_ppm=1000000
```

Fallback ratio:

```text
fallback / total = 2191 / 2191 = 100%
```

This is far above the 1% race threshold. Under the instrumented pre-fix path, every counted client frame wait resumed through the 5 ms fallback and none resumed through explicit compositor upload wake.

## Frame Evidence

Observed frame markers:

```text
Frame #500 near ticks=5000
Frame #1000 near ticks=10000
Frame #1500 before ticks=15000
Frame #2000 after ticks=15000
```

The serial stream stopped after the fourth wake-counter dump, so this phase does not claim a full-run FPS measurement. The counter ratio is sufficient to prove the fallback path is still the normal wake mechanism pre-fix.

