# Input driver coverage

The shared `/dev/input/events` core accepts hardware and virtual devices through
one ABI. Hardware drivers register a driver-neutral capability description with
`input::stream::register_hardware`, retain the returned identity, and hand complete
SYN_REPORT batches of `input::hardware::Event` to `input::stream::publish_hardware` (the hardware handoff’s `report` entry).
The fixed-storage handoff runs without allocation in IRQ context; the input task
publishes records, ledger dispositions, reader wakes and console fallback. A full
handoff or overlong frame produces SYN_DROPPED. Hardware identities below 1024
are not reused during a boot; virtual identities begin at 1024. A detached USB
interface drains its accepted batches before the core releases its held keys,
before any replacement instance’s reports.

## Target coverage

| Target | Keyboard | Pointer | State |
|---|---|---|---|
| x86-metal | `xhci-hid-boot-keyboard` (port 2, interface 0) | `xhci-hid-pointer` (port 2, interface 1, relative); absolute device pending Q1 | Registration implemented; acceptance boot belongs to the later unit. |
| ARM64 with xHCI HID | `xhci-hid-boot-keyboard` | `xhci-hid-pointer`, capabilities from the bound report layout | Same USB drivers and input interface; built only. |
| ARM64 virtio-input MMIO | `virtio-input` keyboard | `virtio-input` tablet, absolute X/Y 0..32767, buttons and relative wheel | Counterpart harvested from e753f9a2; built only. |

The boot keyboard advertises only key codes its `hid_boot::report` decoder emits,
including its eight modifier usages. Each USB pointer interface advertises its
layout's relative or absolute X/Y axes, button usages and wheel. Stream buttons
belong to that interface; the old pointer snapshot merges interfaces on a port.
`input::pointer` remains the atomic USB pointer snapshot for syscall 413's
16-byte form. Virtio's existing tablet snapshot remains available too.

Q1 is open: the x86-metal attached pointer is relative. The shared
`input-devices/pointer-completion-path` case still requires an absolute pointer
and is unchanged. ARM still lacks boot validation of this registration and of the
host-channel transport and later replay work; no ARM boot is authorized for this step.

## Producer and registration map

Reporting and hardware registration callers are listed here. The
host check `tests/input_driver_coverage.rs` scans the kernel source tree against
this map and refuses architecture or virtio coupling in the shared handoff.

| Source | Registration / reporting path | Consumer |
|---|---|---|
| `kernel/src/drivers/xhci/keyboard.rs` | Successful boot-keyboard attach registers decoded key capabilities; completions and typematic submit KEY edges plus SYN_REPORT; detach unregisters the identity. | Shared stream and console fallback. ARM counterpart: the same xHCI HID driver, or virtio-input MMIO. |
| `kernel/src/drivers/xhci/pointer.rs` | Successful HID attach registers layout capabilities; decoded axes, button edges and wheel go through the hardware handoff; motion also calls `input::pointer::report`. | Shared stream plus syscall 413 snapshot. ARM counterpart: the same xHCI HID driver, or virtio-input tablet. |
| `kernel/src/drivers/virtio/input_mmio.rs` | Bound keyboard/tablet instances register capabilities; `input_decode::Frame` retains partial frames across IRQs and submits at SYN_REPORT. | Shared stream and console fallback; existing tablet snapshot retained. USB counterpart: xHCI HID keyboard/pointer. |
| `kernel/src/input/stream.rs` | Defines `register_hardware` and exports `publish_hardware`; hardware and uinput records use the same serial, SYN, grab and ledger machinery. | `/dev/input/events`, compositor reader and console. |
| `kernel/src/keyboard.rs` | PS/2 scancode decoder calls `input::report_key`. | Legacy console-only path; no PS/2 device is required by this step or attached in the retained x86-metal census. |

`kernel/src/input/hardware.rs` drains the bounded handoff in task context.
`kernel/src/drivers/hid_boot/mod.rs` records actual USB console admission after
stream routing; it no longer reports a second copy of decoded keys.


## Serial host-channel virtual devices (E21 W1)

Before launching a suite whose target declares `hostkeys`, the shared input core
creates a kernel-owned host keyboard and host absolute pointer. Their capabilities
are the HID boot decoder's keyboard codes, ABS_X and ABS_Y in 0..32767, left,
right and middle buttons, and signed REL_WHEEL. They use the ordinary input stream,
serials, SYN_REPORT batches, ledger, compositor grab and console fallback. They
are separate from the bound physical devices and do not satisfy Q1's physical
absolute-pointer requirement.

The one line entry is `host::serial_line(&[u8])`, called by
`kernel/src/serial/command.rs` and its ARM counterpart,
`kernel/src/serial_aarch64.rs` (PL011 receive). This entry only tries its bounded
queue lock, never waits, allocates, decodes HID or writes serial output. It
notifies the input task by an atomic generation only. While host input is armed,
that task uses a 1 ms timed wait so no receive IRQ needs a waitqueue or scheduler
lock; ordinary boots retain their event-driven wait. Keyboard reports retain the
previous 10 ms spacing in task context. The input
task decodes accepted reports with `hid_boot/report.rs` and publishes complete
batches through the core, retaining an in-flight batch if ledger pressure refuses
it. Its queue holds 256 reports or pointer samples; a key packet is admitted whole.
The line buffers discard overflow through the next CR or LF.

BXKEY's existing format, sequence and CRC are unchanged (console contract §10).
BXPTR is:

```text
BXPTR @<sequence:16 lowercase hex>:<crc32:8 lowercase hex> <x> <y> <width> <height> <buttons> <wheel>
```

The six payload fields are decimal, separated by single spaces. Coordinates and
mode dimensions are unsigned pixels; wheel is a signed 32-bit delta. Buttons are
a full state mask: bit 0 left, bit 1 right, bit 2 middle; other bits are refused.
Width and height must exactly equal the screen published by the shared GPU core,
and each coordinate must be in bounds. Each dimension must be 1..32768: a larger
mode cannot represent the full pixel range in the advertised absolute-axis domain.

The CRC is reflected IEEE CRC-32 (polynomial 0x04c11db7, initial/final complement),
over the sixteen sequence digits, one space and the exact decimal payload bytes.
The keyboard and pointer have independent sequence streams, starting at one.
Accepted numbered packets and valid retransmissions receive `BXKEY_ACK` or
`BXPTR_ACK` followed by one space and the sixteen sequence digits. A retransmission
is refused as new input but acknowledged again; a gap, old sequence, bad CRC,
mode mismatch, busy lock or insufficient queue capacity gets no ACK and does not
advance sequence. A sender retries a missing ACK with identical sequence and
payload. ACK means the whole packet has entered the kernel's bounded queue.
ACK output is deferred to task context on both transports.

For pixel `x` and dimension `w > 1`, the core chooses
`v = ceil(x * 32767 / (w - 1))`. Thus the consumer's contract formula
`(v - min) * (w - 1) / (max - min)`, with min=0 and max=32767, returns exactly x,
including both edges of 2560x1600. A dimension of one maps its sole pixel to `0`.
The core emits both axes, changed button edges, a nonzero wheel delta, then
SYN_REPORT. It does not alter syscall 413's legacy hardware snapshot.

| Source | Registration / reporting path | Consumer |
|---|---|---|
| `kernel/src/input/host/runtime.rs` | Registers both host devices through `register_hardware`; task-only HID decoding and pointer events use shared stream publication. | Shared stream and console fallback on both architectures. |

The six original BXKEY parser tests moved unchanged from the xHCI driver.
`tests/input_host_channel.rs` and `tests/fixtures/bxptr-vectors.txt` cover pointer
normalization, mode/CRC/sequence refusal and atomic queue admission. ARM still
lacks boot proof of PL011 reception and launcher support for serial host input;
no ARM launcher or host-side BXPTR sender is added by this kernel-only unit.

## Completion replay

`/proc/breenix/input-replay` uses the shared `input::control::Replay` interface.
The endpoint checks the immutable PID-1 suite latch and effective root authority at
open, read and write; a root-open descriptor loses authority after `setuid`.
Production opens return ENOSYS. Procfs registers the pathname so name resolution
can reach the live descriptor instead of a content snapshot.

Selector 1 replays the boot keyboard's A and release reports. Selector 2 encodes
an absolute position and left press/release from the bound HID layout, including
its report IDs, logical ranges and padding. A relative-only layout refuses selector
2; it cannot supply the absolute events required by `pointer-completion-path`.
Q1 and that case are unchanged.

The xHCI worker owns Stop Endpoint, DMA-buffer writes, dequeue advancement,
production completion dispatch and reposting. USB HID on ARM uses that same path.
The ARM virtio-input implementation resets its bound device to recover DMA custody,
then uses the same completion/repost adapter as its used-ring IRQ path before
returning DRIVER_OK. Neither implementation synthesizes a virtual input device.

Bit 8 arms test-control point 33, `INPUT_REPORT_SKIPPED`, for the replay's device.
The press/release pair is one observed transaction: the fault fires once and keeps
both reports silent while decoding and requeue still run. The receipt counts calls
at the shared hardware ingress and reports the bound ring's posted descriptors.
Other devices and reporting threads cannot consume that transaction's fault.

The BreenGL `isolation/test-hooks-absent` addition is coverage of this new authority;
its acceptance PR must name that coverage addition. ARM registration, replay and
PL011 input remain built but unbooted until the operator releases ARM. ARM launcher
serial host-channel transport remains later work.
