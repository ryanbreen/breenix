# Effort suites

An effort suite is a userspace program, `/sbin/suite-<id>`, that the production kernel runs
as PID 1 to show how far one POSIX area works: it runs a list of cases, prints one serial
line per case and draws a scored panel on screen. Vigil reads the manifests here and the
serial lines a suite boot prints.

## Manifest: `docs/suites/<id>.json`

```json
{
  "id": "files-io",
  "title": "Files & I/O",
  "area": "Files & I/O",
  "summary": "One plain sentence: what passing every case demonstrates.",
  "binary": "/sbin/suite-files-io",
  "categories": [
    { "id": "open", "title": "open / creat flags",
      "cases": [ { "id": "creat-excl", "title": "O_CREAT|O_EXCL fails with EEXIST on an existing file" } ] }
  ]
}
```

`area` is the POSIX area name Vigil's syscall page uses. Suite, category and case ids are
lowercase words of `a-z` and `0-9` joined by `-`. The binary runs exactly the manifest's
categories and cases, in the same order and with the same titles. `tests/suite_manifests.rs`
checks every manifest's shape and ids, and checks it against the table the suite runs: it
parses the suite's source, `userspace/programs/src/suite_<id>.rs` with `-` in the id written
as `_`, and reads the `static` that `main` calls `.run()` on, so comments, disabled code and
calls outside that table do not count.

## Serial lines

The suite's stdout, one line each, in this shape:

```text
SUITE <id> START cases=<total>
SUITE <id> CASE <category>/<case> PASS ms=<n>
SUITE <id> CASE <category>/<case> FAIL ms=<n> msg=<short plain reason, no newlines>
SUITE <id> CASE <category>/<case> SKIP msg=<why>
SUITE <id> DONE passed=<p> failed=<f> skipped=<s> total=<n>
```

Each record is one write that starts with a newline, so it always begins its own line even
when other output on the same serial port (the x86-64 scheduler's COM1 breadcrumbs) left a
line unfinished; a reader takes the lines that start with `SUITE <id> ` and ignores blank
lines.

Each case runs in a forked child whose stdout and stderr go to `/dev/null`, so nothing a
case prints can look like a suite line; only the VALUE and WAIT records below, which the
runner writes for it, reach the serial while a case runs. A case that crashes or runs past the suite's time
limit (10 seconds unless the suite sets its own) is a FAIL whose message says so, and so is
one that cannot be started in a child; the suite goes on. The suite waits on a case by
polling, not sleeping, so a monotonic clock that stops is reported as a FAIL too. If the
suite itself stops, the missing CASE lines show where.

### Live measurements: VALUE, WAIT and PULSES

While a case runs it may print three more records, so a reader such as Vigil can show the
numbers behind a result and count down a wait as it happens:

```text
SUITE <id> VALUE <category>/<case> <name>=<number><unit> [expect=<low>..<high><unit>]
SUITE <id> WAIT <category>/<case> until=<ms> for=<ms> result=<name>
SUITE <id> PULSES <category>/<case> every=<us> limit=<us> at=<us>,<us>,...
```

VALUE reports a quantity the case measured and asserts on: a clock reading, a drift in
ppm, how late a sleep ended, a timer's period, an overrun count. `<name>` is lowercase
words of `a-z` and `0-9` joined by `-`; `<number>` is a decimal integer, optionally
negative, optionally with a fraction (`-12`, `340`, `0.5`); `<unit>` is lowercase letters
written straight after it (`us`, `ms`, `ppm`, `hz`, `s`), or nothing for a count. The
optional `expect=` gives the inclusive range the case accepts, in the same unit. WAIT says
the case has started waiting `for` milliseconds on a sleep or a timer, ending at `until`,
CLOCK_MONOTONIC milliseconds (the time since boot); a reader that has no clock of the
guest's counts down `for` from when the line arrived. `result` names the VALUE that says
how the wait ended: the wait is over when the case reports a VALUE of that name, and a
VALUE of any other name reported meanwhile leaves it running. PULSES gives the times a
periodic timer fired, each in microseconds after the case armed it, the programmed period
`every` and how late an expiry may come, `limit`; it carries at most 32 times. None of the
records is a result: the CASE line that follows decides the case.

They are written like the other records, one write starting with a newline. A case prints
them through `value(name, number, unit, expect)`, `wait_for(ms, result)` and
`pulses(every_us, limit_us, at_us)` in `libbreenix::suite`,
which write to a close-on-exec copy of the suite's stdout the case keeps before its own
output goes to `/dev/null`; a program the case execs cannot print them. They name the
running case, appear between its suite's START and DONE, and are bounded: a case reports
the few quantities it asserts on, never one per loop iteration, and the runner drops any
past `RECORDS_PER_CASE` (12) in one case process. Readers that know only START, CASE and
DONE skip them; `scripts/suite-verdict.py` accepts them when they are well formed and name
a manifest case, and does not count them.

The suite's own panel shows them too. Under the category groups, a live strip shows the
clocks as they tick (CLOCK_MONOTONIC to the millisecond, CLOCK_REALTIME as UTC wall time
with a bar sweeping each second), the running case's latest WAIT as a countdown that drains
in real time and then shows the VALUE the WAIT names once the case reports it (how late the
wait ended, say), and the case's latest VALUEs in large digits on gauges: the `expect` range
as a band and the value as a needle, green inside and red outside, with an arrow when it is
off the scale. A case that reports PULSES is also drawn as a pulse train: each expiry at the
time it was observed, against ticks at the programmed period, green when it came no earlier
than its tick and no more than `limit` after it, red otherwise. The strip takes the height the
groups leave, draws larger digits when there is room and leaves out what does not fit on a
small framebuffer; the category bars and the score are unchanged. The case sends each
record to the runner as well, down a close-on-exec, non-blocking pipe, and the runner draws
the strip about 15 times a second while it polls for the case to exit, flushing only the
strip. Nothing is drawn in the case's process, and no tolerance depends on the display. A
disk whose `/etc/breenix/suite-live` reads `off` gets the panel without the strip and no
pipe.

After every case the runner compares CLOCK_REALTIME with CLOCK_MONOTONIC: when CLOCK_REALTIME
has moved more than 100 ms against it since the case started, the runner sets it back and
fails the case saying so, whether the case passed, failed or was killed for running too
long. A case that sets the clock still puts it back itself.

A case can bound its own waits by `case_ms_left()`, the time left before it is killed. When
a case ends, any process it left behind has been reparented to the suite, which runs as
PID 1; the suite kills them all with `kill(-1, SIGKILL)` and reaps them before the next case,
so no case sees another's processes. A case whose processes outlive SIGKILL is a FAIL.

## Writing a suite

A suite is a static table and a `main` that runs it, using `libbreenix::suite`:

```rust
use libbreenix::suite::{case, category, check, skip, suite, CaseResult, Suite};

static SUITE: Suite = suite("smoke", "Smoke", &[
    category("process", "Process and time", &[
        case("getpid", "getpid returns a positive PID", getpid),
    ]),
]);

fn getpid() -> CaseResult {
    let pid = libbreenix::process::getpid()?;
    check(pid.raw() > 0, "getpid returned 0")
}

fn main() { SUITE.run() }
```

A case returns `Ok(())` to pass, and may report what it measures with `value` and
`wait_for` (see above). `?` on a libbreenix error or a string fails it with that
message; `fail(msg)` and `check(condition, msg)` fail it, and `skip(why)` skips it. Add the
binary to `userspace/programs/Cargo.toml` as `suite-<id>` and to `STD_BINARIES` in
`userspace/programs/build.sh`; `scripts/create_ext2_disk.sh` installs `suite-*` in `/sbin`.

## Booting a suite

- ARM64 QEMU: `scripts/boot-interactive.sh --mode suite --suite <id>` (fw_cfg
  `opt/breenix/mode=suite`, `opt/breenix/suite=<id>`).
- Parallels and VMware: `./run.sh --parallels|--vmware --suite <id>`.
- x86-64 gate: `BREENIX_BOOT_SUITE=<id> docker/qemu/run-x86-gate.sh`, or
  `swift run breenix-runs run x86 --suite <id>` from `tools/breenix-runs`. The gate scores
  the boot with `scripts/suite-verdict.py`: one START with the manifest's case count, a CASE
  line for every manifest case in order, a DONE that agrees with them and has `failed=0`, and
  no fatal kernel output. With `BREENIX_QMP_SOCKET=<path>` QEMU also opens a QMP socket for
  screendumps, and the gate saves the final screen as `screen.png` next to the boot's serial
  logs (breenix-runs keeps boot N's as `screen-N.png`).

The last two write `/etc/breenix/boot-target` (one line, `suite <id>`) onto a copy of the
ext2 disk with `scripts/write-boot-target.sh`, which also checks that the copy's
`/sbin/suite-<id>` is the binary just built. On both architectures the kernel takes the
fw_cfg mode first (`opt/breenix/mode=suite` with `opt/breenix/suite=<id>`; on x86-64 QEMU too),
then that file, else the default `/sbin/init`. A suite binary that is missing or not a
loadable ELF for the kernel's architecture runs `/sbin/init` instead, and so does one that
then fails to start.

## The panel

As cases run, the suite draws libgfx's diagnostics panel in its scored form on the
framebuffer: the title, the overall score (passed of total, a percentage and a bar), one
progress bar per category and each case marked pending, running, pass, fail or skip. A
category with more cases than fit shows a window of them that follows the run (the running
case, then failures, then its neighbours) and a row counting the rest. The suite takes the
display back before every update, so a case that took it cannot leave the panel stale.
When every case has run it prints the DONE line, leaves the final panel up and idles; it
never exits. On x86-64 the production kernel gives the display owner the bootloader's
framebuffer (and stops drawing its own log there), so the panel uses that framebuffer.

## Files & I/O

`files-io` has 219 syscall and descriptor cases. Stream stdio cases are deferred until
there is a musl-built helper; libbreenix-libc supplies Rust's runtime ABI and has no
stdio implementation. Synchronization cases call the kernel's fsync/fdatasync ABI,
so an unimplemented syscall fails with ENOSYS rather than passing a libc stub.
The sync category runs before mmap: the raw-disk mapping holders must not
be written back by a later global sync, which could hide an fsync/fdatasync defect.
The poll-select category covers regular files, pipes and FIFOs; sockets belong to
the networking effort. Blocking helpers observe the caller parked through procfs
before publishing data or sending a signal. Filesystem statistics exercise the
kernel statfs/fstatfs ABI that supplies statvfs/fstatvfs.

The suite uses the runner's default 10-second case deadline, including helper children;
there is no shorter fork/exec deadline. This is a hang limit, not a performance target.
A timeout reports a failure to finish, not the result of a POSIX assertion. Allow 1800
seconds on x86 for 219 cases, their kill/reap allowance, boot and panel overhead:

```bash
BREENIX_BOOT_SUITE=files-io BREENIX_GATE_TIMEOUT=1800 docker/qemu/run-x86-gate.sh 1
# From tools/breenix-runs (also captures the final panel through QMP):
swift run breenix-runs run x86 --boots 1 --suite files-io --gate-timeout 1800 --sha <pushed-sha>
```

Suite helpers are ordinary userspace binaries named with the `_test` suffix, built via
`STD_BINARIES` and installed in `/usr/local/test/bin`. They ship alongside the suite in
all images, including default boot images, so changing boot mode does not require a
second userspace image; default boot does not execute them. Files & I/O uses
`/usr/local/test/bin/files-io-exec_test` for descriptor lifetime across exec. Helpers
must return their result to the case and must not print suite serial records.

## Directories & links

`directories` measures the directories effort in `docs/efforts/path.json`. Its
106 cases cover eight categories, one per suite milestone: `mkdir-rmdir`,
`readdir`, `links`, `symlinks`, `rename`, `cwd`, `permissions` and `timestamps`.
The boot-path `userspace-fs` and system-interface milestones are separate measures.

Each case creates its own PID-specific tree on the writable root filesystem and
uses the shared runner's default 10-second deadline. Unsupported operations fail;
there are no skips. Reads reopen filesystem paths, and metadata assertions issue
fresh stat calls. Permission cases clear supplementary groups and drop real UID/GID before checking access bits.
Directory streams are thin unbuffered getdents64/lseek adapters: replay goes back
to the kernel, without a cached entry list. No lexical ordering is assumed.
Cookies are acquired and replayed within the same rewind epoch; POSIX leaves
reusing a pre-rewind telldir cookie unspecified. Timestamp assertions allow ext2's
whole-second resolution. Baseline failures measure kernel gaps and are expected. Linux ABI/policy and ext2
link-count cases are named explicitly; they are implementation checks.

```bash
scripts/boot-interactive.sh --mode suite --suite directories
./run.sh --parallels --suite directories
./run.sh --vmware --suite directories
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode full --boots 1 --suite directories --gate-timeout 1800 --sha <pushed-sha>
```

## Processes

`processes` measures the processes effort in `docs/efforts/path.json`: one category per
suite milestone, `fork`, `exec`, `wait`, `groups-sessions`, `credentials`, `limits` and
`scheduling`. The `wait` category begins with the twelve cases of the former `waitpid`
suite, moved unchanged (same iterations, checks and limits); that suite is retired.

Cases call the kernel by its Linux numbers and assert on the raw return, so an
unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction, for those
calls and for the handlers' rt_sigreturn, so the cases run the kernel's SYSCALL entry and
return path. Library-level interfaces are made as a C library makes them: getrlimit and setrlimit through prlimit64, nice through
getpriority/setpriority, getpgrp as getpgid(0), and fork as clone(SIGCHLD) on ARM64.
Each case uses the runner's default 10-second deadline. Every wait on another process
inside a case is bounded at 3 seconds (6 for an exec) and stops 1.5 seconds before the
deadline, so a failing case still cleans up and reports its own reason. Every process a
case starts is killed when the case ends, and the runner kills and reaps any that remain
before the next case. Cases that need another user switch to a user ID of their own after
starting; the suite itself runs as root.

Where POSIX leaves a value to the system, cases read it rather than assume Linux's: ARG_MAX
is what a C library's `sysconf(_SC_ARG_MAX)` reports on this ABI, getgroups may list the
groups in any order and may include the effective group ID, and an orphan's new parent need
only be a live system process. The three `exec/script*` cases measure `#!` interpreter
scripts, an extension every Unix system provides but POSIX execve does not require (it may
fail with ENOEXEC); their titles say so.

The exec cases run `/usr/local/test/bin/processes-exec_test`, which reports its argv,
environment and the process state exec kept (IDs, process group, signal dispositions,
mask, pending set, descriptors, limits, nice value, working directory, umask, alarm)
on a descriptor named on its command line. Set-ID cases run a copy of it chowned and
chmodded in a per-case directory under `/tmp`.

```bash
scripts/boot-interactive.sh --mode suite --suite processes
./run.sh --parallels --suite processes
./run.sh --vmware --suite processes
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode suite --boots 1 --suite processes --gate-timeout 1800 --sha <pushed-sha>
```

## Signals

`signals` measures the signals effort in `docs/efforts/path.json`: one category per suite
milestone, `dispositions`, `masks`, `handlers`, `waits`, `altstack`, `realtime` and
`job-control`. Interval timers and alarm are in `waits`; kill's targets and permission
rules and the signal state fork and exec keep are in `dispositions` and `masks`.

Cases call the kernel by its Linux numbers and assert on the raw return, so an
unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction, for those
calls and for the handlers' rt_sigreturn, so the cases run the kernel's SYSCALL entry and
return path. Library-level interfaces are made as a C library makes them: sigqueue through rt_sigqueueinfo, sigwaitinfo and sigtimedwait through
rt_sigtimedwait, pthread_kill through tgkill, raise as kill(getpid()), and on ARM64 pause
through ppoll and alarm through setitimer. Handlers are installed with the
`struct sigaction` libbreenix passes to rt_sigaction; `dispositions/sigaction-layout`
checks that call against the Linux ABI's layout (handler, flags, restorer, mask) on its own,
so a layout mismatch fails that case rather than every handler case. Realtime signals are
the kernel's 32 to 64, before a C library reserves any for itself. Where POSIX leaves a
behaviour to the implementation, the case title begins `Linux policy:` and the case measures
what Linux does: a standard signal sent repeatedly while blocked is delivered once
(`masks/not-queued`), and a realtime signal sent by kill is queued (`realtime/kill-queued`).
The queuing POSIX requires, for sigqueue to a signal with SA_SIGINFO set, is measured on its
own (`realtime/queued`, `fifo`, `sigwaitinfo`). `realtime/eagain` sets the queue limit
through Linux's RLIMIT_SIGPENDING.

Each case uses the runner's default 10-second deadline. Waits on other processes are
bounded at 3 seconds (6 for an exec) and stop 1.5 seconds before the deadline; a wait for a
signal that may never come (sigsuspend, pause, sigwaitinfo) is ended by a watchdog child
sending SIGHUP after 3 to 6 seconds, and the case fails when that signal is what ended the
wait, so a lost signal is reported as such. Children that must be blocked before they are
signalled are observed through `/proc/<pid>/status`.

The two register cases hold known values in registers with inline asm and check them in a
loop with no system call in it: the general registers, and all 128 bits of v0-v31 or
xmm0-xmm15. The asm raises a flag once the values are loaded and lowers it before they are
released, and a case needs ten handlers to have run while the flag was up, within five
seconds. The handler overwrites the registers. The x86-64 userspace target is built without
SSE, so compiled code there never uses the xmm registers.
Permission cases switch to user IDs 4242 and 4343 in children; the suite itself runs as
root. Two cases need two processors (`handlers/spinning-target` and
`job-control/stop-threads`). They read the count from /proc/cpuinfo, fail when it cannot be
read, and skip below two. Before measuring, each shows that its two workers run on
different processors at once: a thousand handoffs through shared memory, which two that
take turns on one processor cannot make in the time allowed. None assumes more than two.

The exec cases run `/usr/local/test/bin/signals-exec_test`, which reports the mask,
pending set, ignored and caught signals and alternate stack exec kept on a descriptor named
on its command line, or says there that it runs and then unblocks a signal.

```bash
scripts/boot-interactive.sh --mode suite --suite signals
./run.sh --parallels --suite signals
./run.sh --vmware --suite signals
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode full --boots 1 --suite signals --gate-timeout 420 --sha <pushed-sha>
```

## Time & timers

`time` measures the time effort in `docs/efforts/path.json`: one category per suite
milestone, `clocks`, `sleep`, `timers` and `itimers`. The system-interface milestone is a
separate measure.

Cases call the kernel by its Linux numbers and assert on the raw return, so an
unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction. Library-level interfaces are made as a C library makes them: time() as
the time call on x86-64 and from CLOCK_REALTIME on ARM64, which has no time call, and alarm
through setitimer on ARM64. Signals from timers are taken with sigtimedwait while blocked,
or counted by a handler; the timers' sigevent and the siginfo they deliver use the Linux
ABI's layout. Calls that return 0 when they succeed must return exactly 0.

Every sleep and timer is checked on both sides: it may never end early, and it may end late
by at most two timer ticks and 20 ms. The tick differs per architecture: x86-64 ticks at
200 Hz (5 ms) and ARM64 at 1000 Hz (1 ms), so the bound is 30 ms on x86-64 and 22 ms on
ARM64. `sleep/nanosleep-short` holds the median of twenty 1 ms sleeps to one tick and 1 ms,
and `clocks/coarse-tick` checks that CLOCK_MONOTONIC_COARSE reports and advances in steps
of the tick. Rates are measured over 2 to 3 seconds: CLOCK_REALTIME against
CLOCK_MONOTONIC (100 ppm), CLOCK_MONOTONIC against the processor's own counter (1000 ppm;
ARM64's CNTVCT_EL0 at CNTFRQ_EL0, x86-64's TSC at the frequency CPUID leaf 0x15 gives,
and a skip when the processor does not give one) and CLOCK_REALTIME against the RTC read
through Linux's `/dev/rtc0` RTC_RD_TIME (2000 ppm). CPU-time clocks and timers are
checked to stand still while the caller sleeps and to advance while it computes. A CPU-time
timer is timed in the CPU time it counts, measured across the computing: a POSIX CPU-time
timer by its own clock, ITIMER_VIRTUAL by getrusage's user time and ITIMER_PROF by its user
and system time. It must fire once that CPU time has grown by what getitimer or
timer_gettime said was left when the computing began, and late by at most two ticks and
20 ms of it; the sleep before it makes system calls that ITIMER_PROF and the process
CPU-time clock rightly count, and ITIMER_VIRTUAL, which counts only user mode, may lose no
more than two ticks and 2 ms to that sleep. While the thread CPU-time timer runs, a second
thread computes, so a timer that counted the process's time would fire too soon.
`itimers/virtual-interval` counts expiries of a 20 ms ITIMER_VIRTUAL against the user CPU
time getrusage measured: never more than one per 20 ms of it, and no fewer than the time
less two ticks and 20 ms allows.

Periodic timers are checked expiry by expiry: the k-th must come no earlier than k periods
after arming and late by at most two ticks and 20 ms, and the case reports each expiry's
time as a PULSES record. An interrupted sleep must end once the interrupting signal is sent,
by a child at a set CLOCK_MONOTONIC time, and late by at most the same bound, with the
handler run once. An alarm's death is timed from when the dying process armed it, which it
reports down a pipe. `itimers/alarm-replace` watches past the replaced alarm's deadline.

Cases that set CLOCK_REALTIME put it back before they end, advanced by the time that
passed. `clocks/settime-eperm` sets it as user 4242 in a child. `clocks/monotonic-cpus`
needs two processors: it reads the count from /proc/cpuinfo, skips below two, and first
shows the two threads run at once with a thousand handoffs before comparing their
readings. It assumes no more than two.

Where POSIX leaves a behaviour to the implementation, the case title begins
`Linux policy:` and measures what Linux does: `clocks/monotonic-fine`, `clocks/efault`,
`clocks/coarse-tick`, `sleep/nanosleep-efault`, `sleep/nanosleep-sa-restart` (nanosleep is
never restarted), `itimers/alarm-shares-real` and `itimers/exec-cpu` (exec keeps the CPU
interval timers). `clocks/rate-rtc` reads the RTC through Linux's ABI and its title says so.
POSIX itself requires what the fork and exec cases check: fork clears the child's alarm
and interval timers and gives it none of the parent's timers, exec keeps the time left
until an alarm, and exec deletes the caller's timers.

Each case uses the runner's default 10-second deadline. Waits on other processes are
bounded at 3 seconds (6 for an exec) and stop 1.5 seconds before the deadline. Children
that must be asleep before they are stopped or before the clock is set are observed through
`/proc/<pid>/status`.

The exec cases run `/usr/local/test/bin/time-exec_test`, which reports the interval
timers exec kept and whether a timer ID still names a timer, on a descriptor named on its
command line, and can then stay alive a given time.

Each case reports what it asserts on as VALUE records (how late a sleep or timer ended,
drift in ppm, overruns, CPU time) and each wait of 100 ms or more as a WAIT record naming
the VALUE that ends it.

`clocks/counter-cpus` reads the processor's counter from user mode on every online
processor: one thread per processor in /proc/cpuinfo pins itself there with
sched_setaffinity, reads the counter twice, and checks with getcpu before and after that it
ran on that processor. The two ARM64 trap cases check the counter access user mode is
denied: reading CNTPCT_EL0, CNTV_CTL_EL0 or CNTP_CTL_EL0 kills a process with SIGILL, and a
SIGILL handler sees each trapped read with ILL_ILLOPC at the instruction's address, steps
past it through the ucontext's pc and resumes, while CNTVCT_EL0 and CNTFRQ_EL0 read without
a trap. They skip on x86-64. `clocks/monotonic-steady` stops its 100000 reads when its time is nearly up and
fails saying how long each read took, rather than being killed.

```bash
scripts/boot-interactive.sh --mode suite --suite time
./run.sh --parallels --suite time
./run.sh --vmware --suite time
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode full --boots 1 --suite time --gate-timeout 420 --sha <pushed-sha>
```

## Memory

`memory` measures the memory effort in `docs/efforts/path.json`: one category per suite
milestone, `anonymous`, `protection`, `shared`, `brk` and `locking`. The system-interface
milestone is a separate measure.

Cases call the kernel by its Linux numbers and assert on the raw return, so an
unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction.
Library-level interfaces are made as a C library makes them: sbrk from brk (it fails when
brk leaves the break below the one asked for), posix_madvise through madvise with the
POSIX advice values, and setrlimit through prlimit64. brk and sbrk left POSIX in Issue 6;
the `brk` cases measure what SUSv2 specified for them, including that memory the break
grows over reads as zero. SUSv2 leaves the contents of memory given back and regrown
unspecified, so `brk/regrow-zero` is a Linux policy case.

Faults are taken in the case's own process. A SA_SIGINFO handler records the signal,
si_code and si_addr of each SIGSEGV, SIGBUS and SIGILL in the faulting thread's own record,
and resumes only from three probes at known addresses: a one-byte load, a one-byte store and
a call, each returning a value that says it faulted. A fault anywhere else restores the
default action and kills the case. Every expected fault is checked for its signal, si_code
and si_addr; a SIGBUS past the end of a file may carry BUS_ADRERR or BUS_OBJERR.
`protection/handler-retry` instead has the handler make the page of the byte being written
writable and return, as a garbage collector's write barrier does, and counts the faults; a
fault that is not SEGV_ACCERR at that byte fails the case. The exec cases call into the
mapping and expect SEGV_ACCERR at the call's target. They take their code from a file mapping, so no user-mode cache
maintenance is involved, except `protection/exec-jit`, which writes the code itself and
makes it coherent as a C library's `__clear_cache` does (on ARM64, DC CVAU and IC IVAU from
user mode, which Linux allows).

Children report to the case through pipes and exit codes, never through the memory under
test, and every wait on one is bounded at 3 seconds and stops 1.5 seconds before the
deadline. Files live under `/tmp`, named after the case's process ID, and are removed when
the case ends.

Two cases need two processors (`anonymous/unmap-cpus` and `protection/tlb-cpus`). They read
the count from /proc/cpuinfo, fail when it cannot be read, skip below two and start one
worker thread per processor beyond the first, at most three; none assumes four. The case's
thread pins itself to processor 0 and worker i to processor i + 1 with sched_setaffinity,
and each confirms with getcpu that it runs there. Before measuring, the case and all its
workers make a thousand rounds of handoffs within 750 ms, each worker checking its processor
at every handoff: threads taking turns on one processor hand off at most once per 1 ms
timer tick, so they would need at least 1000 ms. Then, for 50 rounds, the workers read (or write) a page in a loop while the case
unmaps it (or makes it read-only). Each worker has accessed the page at least 100 times
that round before the call, so its processor has the translation in use; an access begun
after the call returned must fault with SEGV_MAPERR (or SEGV_ACCERR) at the byte touched,
one that succeeds is counted as stale, and any other fault fails the case.

Where POSIX leaves a behaviour to the implementation, the case title begins `Linux policy:`
and measures what Linux does: the rest of a mapping's last page is usable
(`anonymous/partial-page`), a free hint is used exactly (`anonymous/hint-free`), memory is
committed only when touched (`anonymous/sparse`), PROT_EXEC is required to run code
(`protection/exec-denied`), touching a mapping past a truncated end raises SIGBUS
(`shared/eof-truncated`), a pipe cannot be mapped (`shared/pipe-enodev`), regrown break
memory reads as zero (`brk/regrow-zero`), a 1 TiB break increase is refused (`brk/huge`),
RLIMIT_DATA also limits private writable mappings (`brk/rlimit-data-mmap`), mlock rounds an
unaligned address to its page and refuses an unprivileged caller past RLIMIT_MEMLOCK (EPERM
at 0, ENOMEM above it), msync of an anonymous mapping succeeds, and posix_madvise refuses
an unaligned address. Cases titled
`Linux ABI:` measure interfaces POSIX does not define: MAP_FIXED_NOREPLACE, unknown
mprotect bits, brk returning the exact break, MADV_DONTNEED, mincore and user-mode cache
maintenance on ARM64.

Resident pages are read from VmRSS in `/proc/<pid>/status`, locked memory from VmLck there,
and free memory from MemFree in `/proc/meminfo`, all as Linux reports them. A mapping's own
resident pages are measured as the drop in VmRSS across munmap of just that mapping, which
holds whether the kernel maps pages when they are touched or at once. `shared/file-msync-read`
shows that MS_SYNC wrote the stores by the disk blocks of a sparse file: fstat's st_blocks
counts blocks on disk, while read() returns cached pages before any sync; it skips if the
stores alone gave the file its blocks. Each case reports what it asserts on as VALUE records: bytes mapped,
resident pages, fault counts, stale accesses, the time a first touch or a copy-on-write
fault takes per page, and the longest munmap or mprotect while other processors used the
page.

```bash
scripts/boot-interactive.sh --mode suite --suite memory
./run.sh --parallels --suite memory
./run.sh --vmware --suite memory
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode full --boots 1 --suite memory --gate-timeout 420 --sha <pushed-sha>
```

## Threads

`threads` measures the threads effort in `docs/efforts/path.json`: one category per suite
milestone, `lifecycle`, `mutex`, `cond`, `rwlock-barrier`, `tls`, `signals` and
`scheduling`. The system-interface milestone is a separate measure.

Threads are measured through the interface a portable C program uses: cases call Breenix's
own C library, `libs/libbreenix-libc`, for every pthread, sched and signal function they
test, not Rust's std::thread. A function the library does not define fails each case that
needs it with `the C library has no <name>`: `userspace/programs/build.rs` reads libc.a's
symbol index and sets `libc_has = "<name>"` for the thread, sched and signal functions it
has, and the suite calls the library's function under that cfg. Cases titled `Linux ABI:`
call the kernel underneath by its Linux numbers (futex, tgkill, rt_sigprocmask,
sched_setscheduler), so a gap in the kernel is told apart from one in the library. The
suite uses raw system calls otherwise only to arrange a case: installing handlers, reading
the clocks and pending sets, pinning threads and setting the clock.

Each case runs its threads in a process of its own below the case's (a trial), which puts
its result on a shared page; the case waits for it with a bound and kills the trial when it
ends. A thread that never returns from a lock or a wait therefore fails the case with the
step it was in, rather than running the case out of time. The case kills and reaps the
trial before it reads the trial's result. Cases whose process must die (a stack
overflow, exit from a thread, a signal's default action) start it the same way and read its
wait status. Every wait is bounded and stops 1.5 seconds before the case's 10-second
deadline.

`lifecycle/exit-tid-word` checks that the threads of a killed process write nothing into
memory after the process is reaped. In each of up to 12 rounds a process makes two threads,
each with its exit-cleared thread-ID word (CLONE_CHILD_CLEARTID, which pthread_join waits on)
in a page of its own, waits until both have started and ends its main thread with exit_group;
50 ms later the case kills and reaps it, maps 32 fresh pages at once, fills them with a
pattern, waits 50 ms and checks every byte. It stops at the first round in which a byte
changed.

Timed waits (pthread_mutex_timedlock, pthread_cond_timedwait, the rwlock timed locks and
FUTEX_WAIT) are checked as the time suite checks sleeps: never before the deadline and late
by at most two timer ticks and 20 ms. A thread blocked on a lock, a condition or a barrier
must come back within 100 ms of its release; that is a liveness bound, not a performance
target. Cases that set CLOCK_REALTIME put it back before they end.

The cases titled `-cpus` need two processors. They read the count from /proc/cpuinfo, fail
when it cannot be read, skip below two and start one thread per processor, at most four; none
assumes four. Each thread pins itself to its own processor with sched_setaffinity and the
threads make a thousand rendezvous within 750 ms before they measure, which threads taking
turns on one processor cannot do. The scheduling cases that compare SCHED_FIFO threads pin
both to processor 0; `mutex/prio-inherit` also needs two processors, so its threads can take
their priorities on processor 1 before they move to processor 0.

Where POSIX leaves a behaviour to the implementation, the case title begins `Linux policy:`
and measures what Linux does: pthread_join of the caller returns EDEADLK and of a detached
thread EINVAL (current POSIX makes both undefined behaviour), detached threads that ended give back their
memory, a thread gets the stack size it asked for and no more (POSIX makes it a minimum), so
`lifecycle/guard-overflow` faults within the stack and guard it asked for, a thread started
normally is SCHED_OTHER with priority 0, a new attribute object is
PTHREAD_INHERIT_SCHED, an unprivileged pthread_setschedparam to SCHED_FIFO with RLIMIT_RTPRIO
at 0 returns EPERM,
and sched_yield hands one processor between two SCHED_OTHER threads. The zero-filled
PTHREAD_MUTEX_INITIALIZER and PTHREAD_COND_INITIALIZER are the Linux ABI's, and so is the
thread pointer `tls/thread-pointer` checks (TPIDR_EL0 on ARM64, the FS base on x86-64), which
compiler TLS is built on. POSIX leaves the
order of destructor calls unspecified, so the destructor cases check rounds and counts, not
the order of keys. Priority inheritance is an option: `mutex/prio-inherit` skips when
sysconf(_SC_THREAD_PRIO_INHERIT) says it is not offered. Robust mutexes are in the base
standard and are not skipped.

Each case reports what it asserts on as VALUE records: how late a timed wait ended, how
soon a woken thread came back, contention and increment counts, barrier cycles, items
through a queue, destructor calls and the time handoffs took; and each timed wait of 100 ms
or more as a WAIT record naming the VALUE that ends it.

```bash
scripts/boot-interactive.sh --mode suite --suite threads
./run.sh --parallels --suite threads
./run.sh --vmware --suite threads
# From tools/breenix-runs:
swift run breenix-runs run x86 --mode full --boots 1 --suite threads --gate-timeout 420 --sha <pushed-sha>
```
