# A.3 u4: period-zero pacing

The u1 ARM baseline measured typing latency median 10.785 ms, max 16.050 ms;
pointer latency median 6.958 ms, max 11.695 ms; and latency under animation
median 12.873 ms, max 281.547 ms (100 ms maximum limit). The last case failed,
so this unit takes the period-estimation branch of the brief. Present latency
and frame floor passed. The baseline is documented in `a3-baseline.md`.

For a compositor reporting period zero, bwm learns the median of its last eight
positive presentation intervals and uses that period to bound its response hold.
Until eight intervals exist it composes immediately. Non-advancing timestamps
and a change of reported period reset the interval history. The nonzero reported
period remains the period used by the existing decision path.

The test-only commit `0d4cd51b` failed on the old pacer: Compose instead of
WaitUntil(296500000). After the change, `cargo test --test bwm_latency_pacing`
passed all 13 tests. Scoped x86 and ARM64 release bwm builds passed without
warnings, using taskpolicy and CARGO_BUILD_JOBS=4. The initial x86 invocation
failed before compilation because its forked-library path was not normalized;
the build passed with its canonical path.

No boot was run in this unit. The deterministic test establishes the pacing
mechanism, not that the baseline animation outlier is resolved. ARM and x86
acceptance boots remain for the acceptance unit.
