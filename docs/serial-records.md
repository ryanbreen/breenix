# Atomic console records

Issue #2411 and ruling W23 require a bounded console write to remain intact
when the kernel writes a log line. One userspace `write()` of up to 4096 input
bytes is admitted as one serial unit. Larger calls are admitted in 4096-byte
units. ONLCR expansion and the screen console's forwarding are assembled
before that admission; the fixed storage holds 8192 expanded bytes plus the
console parser's 64 held bytes.

On x86-64 the existing COM1 lock covers that unit. On ARM64 the UART ticket
is pinned until the unit has been sent. Pinning and stall takeover use the
same atomic word, preventing a waiter from revoking a ticket after it was
pinned. A previously skipped breadcrumb's byte in flight finishes before the
new record starts. Kernel output waits for a descheduled record owner to
resume. Same-CPU fatal reentry retains the UART's existing reentry behavior.

ARM64 raw diagnostics have a separate writer identity so a partial fork
breadcrumb cannot prefix the next formatted signal-termination line. Their
bytes remain on serial.

## Cause on the unfixed main kernel

These references describe commit `52e0c303`:

- `kernel/src/tty/driver.rs:929`: the TTY divides a buffer into lines and
  256-byte units. Its `serial_writer::exclusive` excludes other TTY writers
  and console echo, while kernel output uses the port directly
  (`kernel/src/serial_writer.rs:20`).
- `kernel/src/console/records.rs:90`: screen forwarding also divides output
  into 256-byte chunks before invoking the serial callback.
- x86-64: `kernel/src/serial.rs:827` takes `SERIAL1` for one such unit;
  `_print` takes the same lock at `kernel/src/serial.rs:897`. Kernel output
  can acquire it between units of one userspace buffer.
- ARM64: `kernel/src/serial_line.rs:451` feeds a TTY write through a buffer
  that flushes at LF or 1024 bytes. In addition,
  `kernel/src/uart_owner.rs:159` permits a waiter to revoke a UART ticket
  after 100 ms without progress. The evicted owner resumes its remaining
  bytes after competing output. This can split even a short suite record.

## Guest regression check

```sh
export CARGO_BUILD_JOBS=4 BREENIX_QUIET=0
scripts/boot-interactive.sh --mode program \
  --program /usr/libexec/breenix/serial-record_test --display \
  --serial-log target/serial-record-test/serial.txt
python3 scripts/check-serial-records.py target/serial-record-test/serial.txt
```

The guest opens `/dev/console` directly, avoiding the program launcher's stdout
pipe. Two children write 192 records of 80, 1024 and 4096 bytes, one syscall
per record, while the parent starts and reaps 128 SIGTERM victims. The checker
reads the unmodified UART capture: it requires the expected record bytes and
multiplicities, one intact signal line per victim, and a signal line between
the first and last record. Partial writes fail in the guest.

The check was committed before the kernel repair (`f04efbda`). Its first
ARM64 QEMU boot rejected 76 corrupted records. On the final production build,
Vigil `20261024T143206Z-qemu-2414`, it accepted 192 intact records and 128
intact signal lines with overlap observed. The suite producer, strict suite
scorer and signal-termination producer retain their existing code.

The host UART check also forces an owner to stall past the takeover bound in
both record orders, and checks the already-in-flight breadcrumb case.
