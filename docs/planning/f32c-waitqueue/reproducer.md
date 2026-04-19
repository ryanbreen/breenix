# F32c WaitQueue Reproducer

## Purpose

`wait_stress` is an opt-in userspace reproducer for the F32 waitqueue race. It
uses a dedicated kernel waitqueue exposed through FBDRAW test ops 27-30 so it
can stress `WaitQueueHead::prepare_to_wait()` and `schedule_current_wait()`
without BWM, window registry state, or compositor readiness conditions.

The reproducer intentionally waits without a persistent condition. That is the
minimal Linux waitqueue race case: a wake that lands after queue enrollment must
make the later schedule operation a no-op. If wake delivery observes the waiter
before the scheduler state is set, the waiter can sleep forever.

## How to Run

```bash
BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 90
```

`BREENIX_WAIT_STRESS=1` makes `scripts/create_ext2_disk.sh` create
`/etc/wait_stress.enabled`. ARM64 init checks for that file and runs:

```text
/bin/wait_stress 60
```

before starting BWM. Normal Parallels validation boots are unchanged when the
flag is absent.

## Phase 1 Evidence

Run:

```bash
BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 90
```

Serial evidence from `/tmp/breenix-parallels-serial.log`:

```text
[init] wait_stress enabled; starting 60s waitqueue stress
[spawn] path='/bin/wait_stress'
WAIT_STRESS_START duration=60s sample=100ms
WAIT_STRESS: forked waiter pid=3
WAIT_STRESS: forked waker pid=4
WAIT_STRESS_STALL sample=3 entered=269 returned=268 wakes=17280 waiters=0
[syscall] exit(4) pid=2 name=wait_stress
[init] wait_stress exited pid=2 code=4
[init] Boot script completed
```

Interpretation: wakes continued (`wakes=17280`) while one wait remained
unreturned (`entered=269`, `returned=268`). `waiters=0` shows the wake side had
already drained the queue entry, matching a lost wake between queue enrollment
and the later blocked scheduler state.

## Validation

These checks passed with no compiler warnings:

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

userspace/programs/build.sh --arch aarch64

cargo build --release --features testing,external_test_bins --bin qemu-uefi
```
