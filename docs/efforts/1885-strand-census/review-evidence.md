# Dispatch census review repairs

The review of #1907 identified two timer classifications that could hide lost wakes and an unbounded wait for a sampled thread's owner lock. The repairs keep a missing `BlockedOnTimer` deadline and a deadline at or before the snapshot time stranded, and sample the owner lock within the existing `OWNER_TRIES` budget of 256 nonblocking attempts. An unavailable sample remains stranded unless a later ledger read observes a restore or exit.

## Host validation

The behavioral test includes the production census module, supplies controlled scheduler samples, and checks the emitted stranded count, TID list, and waiting count. Its cases cover a missing timer deadline, a deadline 500 ms overdue, an exact deadline, a future deadline, indefinite child and I/O waits, an expired signal wait, Ready, Running, terminated, and unavailable scheduler rows.

| Test target | Result |
| --- | --- |
| `dispatch_strand_census_behavior` | 1 passed |
| `dispatch_strand_census_structure` | 7 passed |
| `dispatch_fact_census_structure` | 7 passed |
| `x86_gate_verdict_test` | 35 passed |

The release build with `testing,external_test_bins` passed without warnings. Mac builds ran under background scheduling with four Cargo jobs. These checks ran once on the repaired code before its commits were pushed.

The earlier ARM64 failure is tracked by #1873. The changed census and wait lookup are x86-only, so that boot was not repeated.

Commands used for the affected host targets and release build:

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b cargo test --test dispatch_strand_census_behavior --test dispatch_strand_census_structure --test dispatch_fact_census_structure --test x86_gate_verdict_test
taskpolicy -b cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

## Bounded sampling

The first nonblocking sampler tried the owner lock once. Boot `20261018T153808Z-metal` at `8366c736` reached login, but 2 of 193 captured snapshots reported `stranded=1:tids=24:waiting=1` (seq 43 at 44268 ms and seq 105 at 106293 ms). Thread 24 was getty, parked reading the console. That run was stopped with exit 143 and confirmed power-off rather than accepted as the ten-minute result.

A single acquisition attempt can collide with an ordinary scheduler pass. The revised sampler uses the scheduler's existing `OWNER_TRIES` budget of 256 nonblocking attempts, also used by its owner-lock lookups. For local and remote owners, it calls `try_remote` with the owner CPU, so an unavailable lock advances the finite retry loop. Exhausting that budget still leaves the sample conservatively stranded. This changes acquisition retry policy, not the timer deadline predicate.

The bounded sampler passed the affected release build without warnings. The unchanged census-format, classification, and consumer host tests were not repeated for the acquisition-loop change. The 4 of 4 files in the clean merge of #1912 were clock/workqueue validation files, outside the census, wait-sampling, and production clock implementation; that merge did not trigger another local test or build run.


## Completed production session

Boot `20261018T155716Z-metal` at `e8b76498` ran the bounded sampler in the production console session with the original ten-minute idle limit. Its production image build passed the warning checks and exited 0. The complete serial capture contains 606 of 606 snapshots with `stranded=0`, sequentially numbered 1 through 606; the 603 of 603 snapshots after login (seq 4 through 606, 4801 through 606965 ms) show `saved=2:waiting=2`. The capture reaches `[console] input idle 10 min`. The driver closed with status 0, observed power-off (`off=true`) after 9.7 s, and released its lease.

The selected census records and driver outcome are in [census-excerpt.txt](census-excerpt.txt). The first one-shot sampling result was not accepted; this completed bounded-sampling session supplies the production result for landing. No timer deadline was relaxed to obtain it.
