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
case prints can look like a suite line. A case that crashes or runs past the suite's time
limit (10 seconds unless the suite sets its own) is a FAIL whose message says so, and so is
one that cannot be started in a child; the suite goes on. The suite waits on a case by
polling, not sleeping, so a monotonic clock that stops is reported as a FAIL too. If the
suite itself stops, the missing CASE lines show where.

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

A case returns `Ok(())` to pass. `?` on a libbreenix error or a string fails it with that
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
framebuffer (and stops drawing its own log there), so the panel shows on every platform.

## Files & I/O

`files-io` has 159 syscall and descriptor cases. Stream stdio cases are deferred until
there is a musl-built helper; libbreenix-libc supplies Rust's runtime ABI and has no
stdio implementation. Synchronization cases call the kernel's fsync/fdatasync ABI,
so an unimplemented syscall fails with ENOSYS rather than passing a libc stub.

The suite uses the runner's default 10-second case deadline, including helper children;
there is no shorter fork/exec deadline. This is a hang limit, not a performance target.
A timeout reports a failure to finish, not the result of a POSIX assertion. Allow 1800
seconds on x86 for 159 cases, their kill/reap allowance, boot and panel overhead:

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
