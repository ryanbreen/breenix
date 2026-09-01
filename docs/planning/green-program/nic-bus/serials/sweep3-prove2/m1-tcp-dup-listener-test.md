# M1 proof -- new regression test `tcp_dup_listener_test`, mutation-verified

The review's #724 finding M1 ("dup()/dup2()/F_DUPFD never
tcp_listener_ref_inc, so sys_close's new dec can retire a live listener out
from under a surviving dup'd fd") shipped in a6679e7c with no dedicated
regression test (fix2-notes.md discloses this honestly, citing the
`cargo test`-infeasibility of the `#![no_std]` kernel crate -- confirmed
independently: `cargo test -p kernel --lib` fails to compile, duplicate
`panic_impl`/`alloc_error_handler` lang items against `std`'s).

Added a real, boot-based, gate-observable regression test:
`userspace/programs/src/tcp_dup_listener_test.rs`, wired into the x86
production feature profile the same way `tcp_socket_test` is (Cargo.toml
`[[bin]]`, `kernel/src/boot/test_list.rs::TEST_BINARIES`,
`userspace/programs/build.sh`'s `STD_BINARIES` install list -- a SEPARATE
hardcoded array from Cargo.toml that actually controls what gets packed onto
the disk image; missing this caused the first gate attempt to panic with
`FATAL: DISK LOADING FAILED` -- see the first attempt below), and
`kernel/src/main.rs`'s x86 `RING3_SMOKE` test-launch block.

The test: bind+listen on port 9110, `dup()` the listener fd, close the
ORIGINAL fd, then prove the SURVIVING dup'd fd is still a live, working
listener via a real loopback connect+accept round trip. Then closes the
surviving fd too and proves the listener genuinely retires (a fresh bind to
the same port must now succeed) -- catching a leak in the OPPOSITE
direction, not just the M1 hazard itself.

## GREEN: full x86 gate, beast, a6679e7c + this test wired in

`docker/qemu/run-x86-gate.sh 1 full`:
```
[gate] Build clean (0 warnings) in 16s
x86 userspace gate: PASS - exited=20 expected>=10 nonzero=0 allowlist=0
  Test 1: PASS
GATE: PASS (1/1 boot tests passed; mode=full build=16s boot=150s total=173s)
```
(exited=20, up from the pre-existing baseline of 19 -- the new test ran and
exited 0, folded into the tally.)

The test's own serial output (`serial_user.log`):
```
Step 4: connect+accept through the SURVIVING dup'd fd...
  PASS: accepted a connection through the dup'd fd after the original closed

Step 5: close the last fd; the listener must now actually retire...
  PASS: port 9110 was free after the last fd closed (listener genuinely retired)

=== All TCP dup'd-listener tests passed! ===
TCP_DUP_LISTENER_TEST_PASSED
```
Kernel-side confirmation (`serial_kernel.log`):
```
[ INFO] kernel::userspace_test: ✓ Loaded 'tcp_dup_listener_test' from test disk (188848 bytes)
[ INFO] kernel::process::manager: Created process tcp_dup_listener_test (PID 10)
[DEBUG] kernel::task::process_task: Process 10 'tcp_dup_listener_test' (thread 20) exited with code 0
```

## RED: mutation falsification -- revert the M1 inc-side fix, same test now fails

Reverted exactly the arm this test exercises (`dup_at_least`'s TcpListener
increment case in `kernel/src/ipc/fd.rs`, the path `io::dup()` ->
`sys_dup` -> `FdTable::dup()` -> `dup_at_least()` actually uses) to reproduce
the pre-fix #724-review bug, rebuilt (clean, 0 warnings), booted once
(userspace unchanged, no repack needed):

```
Step 4: connect+accept through the SURVIVING dup'd fd...
  FAIL: the dup'd listener fd did not survive closing the original fd -- the listener was retired early (M1 regression)
TCP_DUP_LISTENER_TEST_FAILED
```

Exactly the predicted failure mode and message. Restored `fd.rs` to its
committed a6679e7c state (`git diff --stat` empty afterward) and rebuilt
clean before continuing.

## Files

- `userspace/programs/src/tcp_dup_listener_test.rs` (new)
- `userspace/programs/Cargo.toml` (`[[bin]]` entry)
- `userspace/programs/build.sh` (`STD_BINARIES` entry -- the missing piece
  the first attempt surfaced)
- `kernel/src/boot/test_list.rs` (`TEST_BINARIES` entry, shared doc list;
  silently skipped on aarch64 per that list's own docstring since no
  aarch64 `.elf` is built for it)
- `kernel/src/main.rs` (x86 `RING3_SMOKE` launch block, mirrors
  `tcp_socket_test`'s wiring)
