# Console serial records

Issue #2411 and ruling W25 require non-fatal kernel output to follow a
userspace console unit. A unit ends at the last newline within 4096 input
bytes; a longer line is cut at 4096. ONLCR expansion and screen forwarding
are assembled before serial admission.

Record exclusion is separate from UART ownership. While a unit is admitted,
competing non-fatal records enter a bounded deferred ring. Its storage is
32 KiB and 64 record descriptors. Adjacent unfinished raw diagnostic
fragments share a descriptor; formatted lines keep their separate descriptors. Admission uses a non-waiting try operation;
a full or contended ring counts rejected records and emits one overflow line
after retained records. Userspace units instead apply backpressure outside
masked sections. The transmitter drains deferred lines before reopening
admission. Hardware ownership and interrupt masking cover sixteen-byte
pieces, with the incoming IRQ state restored between pieces. Thread-side
terminal transmission temporarily enables IRQs when called from a masked
syscall, restoring that state afterwards.

ARM64 retains its terminal transmit queue and the raw writer's bounded
stall recovery. Formatted kernel lines use deferral; raw diagnostics keep a
separate writer identity. Fatal output bypasses both admission and another
CPU's ownership. It may split a unit. Panic context and EL1 fatal reports
select that path; the fatal-report guard tries once rather than waiting.

On x86-64, COM1 admission also excludes the existing kernel log-ring drainer
until the unit ends. SERIAL1 protects each short enqueue piece, including
when a full TxQueue must make room. The unit owner then drains deferred
records and the older kernel log ring. Panic banners use direct emergency
output.

## Checks

The existing guest race opens `/dev/console` directly. Two children write
192 records of 80, 1024 and 4096 bytes, each with one complete write, while
the parent starts and reaps 128 SIGTERM victims. The raw-UART checker
requires the exact record multiset, one intact signal line per victim, and
signal evidence between the first and last records. It is also the host
oracle for the `console/serial/records` suite case; a guest PASS alone does
not satisfy that case's suite verdict.

```sh
export CARGO_BUILD_JOBS=4
scripts/boot-interactive.sh --mode suite --suite console \
  --case serial/records --gpu-gl --display \
  --serial-log target/serial-record-test/serial.txt
```

The host UART tests retain the original stalled-owner assertions. Additional
cases require fatal output to reach the wire while another CPU holds the
UART, and require a signal writer called during a paused record to return
before that record resumes, with its complete line following the record.
The unit-boundary checks cover buffered short lines crossing byte 4096 and
a single longer line.

<!-- claim-lint:ok: #2424 W25 defines these acceptance requirements; this section does not claim completed runtime evidence. -->
The W25 ARM64 acceptance includes the testing boot path, this record case,
the unchanged stall check, and a window-manager comparison against main.
The #2426/#2427 classification uses three GL testing boots on each side.
x86 acceptance requires a separate R240 serial check and server/logs run
under the shared tools lock. A build does not replace that runtime proof.
