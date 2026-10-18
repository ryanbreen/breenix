# PR #1967 retained evidence

These are the original artifacts for kernel commit
`62772f08cef3ec4e14fa02255786df9a150cdee2`, captured on 2026-10-18,
19:40:37–19:42:22 UTC. Publishing them does not represent a new boot or test run.

## Pointer and display

The ARM64 strict boot used QEMU TCG, cortex-a72, four CPUs, 512 MiB and an
open Cocoa window. The retained scratch [driver](driver.py) sent QMP absolute
pointer events through a second socket while boot tests ran.
[driver.log](driver.log) and [driver.json](driver.json) record 403 sweep moves
(the six parking moves are additional) and six successful screendumps.

The six lossless PNGs preserve the original PPM pixels. The retained
[analyzer](analyze.py) and [analysis.json](analysis.json) record one exact
12×18 cursor sprite per frame and the panel background at y=537–718, below
the stage rows ending at 528 and above the status line at 728. Exact sprite
matching covers sampled complete sprites, with no assertion about partial
remnants between captures.

- [000 during](000-during.png)
- [001 during](001-during.png): 78%, 33%, 88%; 108/121 tests; eight complete subsystems
- [002 during](002-during.png)
- [003 during](003-during.png)
- [004 after tests](004-after-tests.png)
- [005 final](005-final.png): 121/121 tests; eleven complete subsystems

To inspect the original analysis without a boot, install Pillow and NumPy,
convert the PNGs back to PPM in a scratch directory, then pass that directory
to `python3 analyze.py`. The script writes its analysis and PNG conversions
there. [SHA256SUMS](SHA256SUMS) identifies the published artifacts.

## Serial and gate result

[serial.txt](serial.txt) contains `[TESTS_COMPLETE:121/121]`,
`[BOOT_TESTS:PASS]`, `[EXEC_SMOKE:TARGET_OK]` and subsequent heartbeats.
[gate.log](gate.log) preserves the **FAIL** verdict, including
`migration_refused=1`, tracked separately in
[#1873](https://github.com/ryanbreen/breenix/issues/1873).
This is display evidence, not a claim that the strict gate passed.

[gate-preflight-attempt.log](gate-preflight-attempt.log) records 68/70 suites
passing. The two failures are retained in
[block_request_lifetime_structure.log](block_request_lifetime_structure.log)
([#1901](https://github.com/ryanbreen/breenix/issues/1901)) and
[dma_and_log_sink_structure.log](dma_and_log_sink_structure.log)
([#1950](https://github.com/ryanbreen/breenix/issues/1950)). The subsequent boot
reused that preflight run rather than rerunning the suites. These failures
are outside the display diff.

## Build results

- [build-arm64-tip.log](build-arm64-tip.log): release kernel completed; only the pinned core notice (#945).
- [check-x86-bt.log](check-x86-bt.log): boot_tests check completed.
- [check-x86.log](check-x86.log): interactive check failed on the unrelated PTY return type (#1962).
- [check-x86-scratch.log](check-x86-scratch.log): diagnostic interactive check with the temporary PTY edit; that edit was not committed and this is not an unmodified-tree pass.

Build and gate logs replace local filesystem locations with placeholders;
serial bytes and screenshot pixels are preserved. No x86 QEMU boot is part
of this evidence. The original Mac boot is recorded in Vigil.
