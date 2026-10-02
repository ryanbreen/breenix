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
lowercase words of `a-z` and `0-9` joined by `-`, at most 40 characters. The binary runs
exactly the manifest's categories and cases, in the same order and with the same titles.
`tests/suite_manifests.rs` checks every manifest's shape and ids, and checks it against the
`category(...)` and `case(...)` calls in the suite's source, `userspace/programs/src/suite_<id>.rs`
with `-` in the id written as `_`.

## Serial lines

The suite's stdout, one line each, in this shape:

```text
SUITE <id> START cases=<total>
SUITE <id> CASE <category>/<case> PASS ms=<n>
SUITE <id> CASE <category>/<case> FAIL ms=<n> msg=<short plain reason, no newlines>
SUITE <id> CASE <category>/<case> SKIP msg=<why>
SUITE <id> DONE passed=<p> failed=<f> skipped=<s> total=<n>
```

Each case runs in a forked child: one that crashes or runs past the suite's time limit
(10 seconds unless the suite sets its own) is a FAIL whose message says so, and the suite
goes on. If the suite itself stops, the missing CASE lines show where.

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
  `swift run breenix-runs run x86 --suite <id>` from `tools/breenix-runs`. With
  `BREENIX_QMP_SOCKET=<path>` QEMU also opens a QMP socket for screendumps, and the gate saves
  the final screen as `screen.png` next to the boot's serial logs.

The last two write `/etc/breenix/boot-target` (one line, `suite <id>`) onto a copy of the
ext2 disk with `scripts/write-boot-target.sh`. The kernel takes the fw_cfg mode first, then
that file, else the default `/sbin/init`.

## The panel

As cases run, the suite draws libgfx's diagnostics panel in its scored form on the
framebuffer: the title, the overall score (passed of total, a percentage and a bar), one
progress bar per category and each case marked pending, running, pass, fail or skip. When
every case has run it prints the DONE line, leaves the final panel up and idles; it never
exits. The production x86-64 kernel has no framebuffer for userspace (its graphics syscalls
are built only with the `interactive` feature), so on x86-64 a suite prints its serial
lines and draws nothing.
