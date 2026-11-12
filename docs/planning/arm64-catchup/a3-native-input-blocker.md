# A.3 native input acceptance: retained-capture audit

<!-- claim-lint:ok: #2901; retained 20261112T091724Z-metal exit 143. -->
Boot `20261112T091724Z-metal` is an incomplete, failed x86 native
window-manager run, not ARM64 QEMU proof. Runtime is `9f665106`, tools
`f178dd7e`; exit status is 143. No retry was made during this audit.

## Capture and measured deadlines

<!-- claim-lint:ok: #2901; retained capture digest and counts recorded in this paragraph. -->
The local Vigil `serial.txt` is exactly 8,388,608 bytes and ends inside a
`VALUE wm-case-budget` record. The retained boot-host file is 11,270,169
bytes; the local file is its identical prefix. The full file has SHA-256
`1a642e5f8cadbd41995ce08cde4e8eaba44aa384f2aeda3aed25c6ae3caf885a`.
It contains 45 complete case verdicts: 26 PASS and 19 FAIL. There is no
`SUITE window-manager DONE`, no final `MEMORY reclaimed=` verdict, and no
`SUITE_SEQUENCE READY window-manager`. The one MEMORY line is the baseline.
The full file also ends in an incomplete accounting record. Neither file
proves completion.

The first failure is `stacking/click-raises`, case duration 2455 ms.
Its retained wait record has begin `498947309798`, deadline
`499947309798`, and end `499948477040` ns: elapsed 1001.167242 ms against
1000 ms, or -1.167242 ms headroom, without client acknowledgment.
At the midpoint (`499452026464` ns), the compositor and both clients are
blocked in poll; the observer is running in write. This snapshot does not
include the kernel input-service thread.

The host records packet-11 retries at Unix times 1794476404.003,
1794476404.421 and 1794476404.840; the last event abandons the packet.
Two complete packet-11 ACK lines exist in `serial.raw`, at byte offsets
5692582 and 5697442 (offsets include the preceding newline). Both match
the exact regular expression in `Driver.wait_replay_ack`.
Their cleaned-file offsets are 5606630 and 5611414. `serial.ts` associates
these with archive batches at 1794476409.497 and 1794476410.007,
respectively, 4657 and 5167 ms after abandonment. These are archive-batch
times, **not** independent raw-arrival or guest-admission timestamps.

The remaining failures cover restore/minimized visibility, focus, pointer
delivery, dragging and resizing, each reporting an unchanged 1000 ms wait.
This is failed evidence, with no single-pass or margin-resolution claim.

## Source and unresolved mechanism

- `scripts/metal/metal-boot`, `start_hostkey_reader`, consumes raw serial
  independently of the archive loop. `replay_send` sends a numbered packet
  at most three times; `wait_replay_ack` scans the latest 65536 raw bytes for
  350 ms per attempt. `pump_serial` stamps a batch before processing it.
  Therefore a later archive stamp cannot distinguish late raw arrival from
  a timely ACK missed by the sender.
- `kernel/src/input/host.rs`, `Channel::accept`, accepts a new packet once
  and ACKs duplicate sequences without adding input again.
  `kernel/src/input/host/runtime.rs`, `serial_line`, defers ACKs and packets
  to fixed queues. `drain_one` emits the ACK before constructing and
  publishing the selected packet, while holding the service mutex.
  Unticketed packets such as packet 11 produce no `HOSTINPUT_ACK` guest
  timestamp. Queue admission, service execution and publication times
  cannot be recovered from the ordinary ACK text.
<!-- claim-lint:ok: #2901; kernel/src/serial.rs TX_BUFFER_SIZE and init baud configuration, retained capture counts. -->
- `kernel/src/serial.rs` serializes COM1 writes through `SERIAL1` and its
  65539-byte transmit queue. A full queue makes writers wait for UART room.
  At configured 115200 baud, 8N1, that much wire data takes about 5689 ms.
  This permits substantial request/ACK transport delay; it does not prove
  queue occupancy at packet 11. The first failing case emits 250413 cleaned
  bytes between its START and the next START, including 1997 VALUE lines.
- `libs/libbreenix/src/suite.rs`, `record`/`emit_to`, puts HOSTPOINTER requests
  on the transcript; `dump_stall_evidence` exports retained accounting to
  the same transcript after a case. `Desktop::focus` emits press and release,
  then checks actual client delivery. Neither its deadline nor the host's
  retry contract should be weakened.

Serial backpressure is a concrete candidate. It is not established whether
the request reached the host after the client deadline, the host missed a
timely raw ACK, the input service was held up before publication, or delivery
stalled between publication and the client. A test that merely models one
candidate would not establish why this boot failed. No speculative test or
runtime fix was committed.

## Ledger blocker and next question

Stop under the mechanism-first rule. One instrumented native window-manager
measurement, only after a coordinator ruling, should answer:

For packet 11's equivalent first missed click, at which boundary does the
1000 ms budget expire: guest request enqueue/UART transmission, host raw
request recognition and send, guest RX admission/input-service publication,
or compositor/client delivery?

Use bounded sequence-correlated guest timestamps and serial queue occupancy,
including ACK enqueue and publication, and host timestamps at raw request/ACK
recognition and send (independent of archive processing). Retain the first
missed click's rows through teardown. This distinguishes the candidates
without extending deadlines. A forcing deterministic check and a source fix
follow only an established mechanism; final ARM/native acceptance follows
only when the runtime fix and coordinator stop conditions permit it.

<!-- claim-lint:ok: #2901; audit at e98c51b8, coordinator lines 307 and 313 distinguish ARM and native evidence. -->
No build, test, boot, deployment, board update or merge was performed in this
audit. The retained ARM run remains the separate coordinator-accepted record
at `9f665106`, `20261112T085617Z-qemu-94194` (ledger line 307); it does not
turn this native run into proof. PR #2874 remains blocked by native-input
issue #2901; animation issue #2872 remains separate and open.

Coordinator line 313 supersedes the unresolved-mechanism and measurement-boot
proposal above: remove the investigation exports from the UART input path.
See [the removal inventory](a3-scaffolding.md). This audit remains the historical
record of the failed capture; acceptance belongs to the subsequent unit.
