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
later host-channel and replay work; no ARM boot is authorized for this step.

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
