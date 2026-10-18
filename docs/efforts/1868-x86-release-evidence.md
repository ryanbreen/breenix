# x86 release kernel optimization: scope and evidence

PR #1880 sets `profile.release.package.kernel.opt-level = 3` because the
x86 kernel is a Cargo build-dependency target artifact. It changes compilation,
not scheduling policy, clock calibration or test deadlines.

The release builds completed without warnings or errors. The diagnostic
`20261018T114534Z-1863-optimized-x86-boot-tests` emitted loopback PASS with
setup 2046 ms, data 251 ms, EOF 394 ms, load 10026 ms and watchdog overrun
101 ms. This was an instrumented working tree containing the profile repair;
its recorded clean commit 52b02620 does not identify those diagnostic bytes.
It is diagnostic evidence, not pristine-commit acceptance.

The final committed stack 8cb40512 includes the separate pathname fast path
in PR #1881. Record `20261018T120541Z-1863-acceptance-x86-boot-tests`
measured loopback data 6 ms and EOF 21 ms, DNS configuration 4 ms and total
DNS 42 ms. It panicked at the independent clock calibration lower bound
(#1859), before loopback load/watchdog completion and
`USERSPACE TEST REPORT DONE`. There is no aggregate gate pass.

All three historical 1863 records were misclassified as aarch64; their
ARM64 scores provide no acceptance evidence. Issue #1883 tracks architecture,
dirty provenance and score correction. The raw x86 serial observations above
remain observations; neither a wrong architecture score nor the interrupted
run establishes a completed target boot.

The one-vCPU diagnostic and four-vCPU final run are different configurations.
They establish no matched before/after comparison. No new runtime changes
are made during landing, so the same code is not tested again. The clock
failure is tracked independently and is outside the build-profile repair.
