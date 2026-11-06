//! Serial IRQs only admit into fixed storage. The existing input task owns
//! decoding, publication and acknowledgement output on both architectures.
use super::{Ack, Channel, Packet};
use crate::input::{
    hardware::{Description, Device, Event},
    stream,
};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use input_abi::{axis, key, kind, Capability};
use spin::{Mutex, Once};

struct Devices {
    keyboard: Device,
    pointer: Device,
}
static DEVICES: Once<Devices> = Once::new();
static ARMED: AtomicBool = AtomicBool::new(false);
struct Receiver {
    channel: Channel,
    acks: [Option<Ack>; super::BACKLOG],
    ack_head: usize,
    ack_count: usize,
}
static RECEIVER: Mutex<Receiver> = Mutex::new(Receiver {
    channel: Channel::new(),
    acks: [None; super::BACKLOG],
    ack_head: 0,
    ack_count: 0,
});

pub fn armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// Task-context initialization for a suite with a hostkeys declaration. x86's
/// serial task and ARM's PL011 receive path use the same devices and ingress.
pub fn init() -> Result<(), &'static str> {
    if armed() {
        return Ok(());
    }
    let mut keys: Vec<_> = (0..=255)
        .filter_map(crate::drivers::hid_boot::report::usage_code)
        .collect();
    keys.extend([29, 42, 56, 125, 97, 54, 100, 126]);
    keys.sort_unstable();
    keys.dedup();
    let capabilities = keys
        .into_iter()
        .map(|code| Capability {
            kind: kind::KEY,
            code,
            minimum: 0,
            maximum: 1,
        })
        .collect();
    let keyboard = stream::register_hardware(Description {
        capabilities,
        keyboard: Some(crate::input::Device::HostKeyboard),
    })?;
    let capabilities = alloc::vec![
        Capability {
            kind: kind::ABS,
            code: axis::X,
            minimum: 0,
            maximum: 32767
        },
        Capability {
            kind: kind::ABS,
            code: axis::Y,
            minimum: 0,
            maximum: 32767
        },
        Capability {
            kind: kind::KEY,
            code: key::BUTTON_LEFT,
            minimum: 0,
            maximum: 1
        },
        Capability {
            kind: kind::KEY,
            code: key::BUTTON_RIGHT,
            minimum: 0,
            maximum: 1
        },
        Capability {
            kind: kind::KEY,
            code: key::BUTTON_MIDDLE,
            minimum: 0,
            maximum: 1
        },
        Capability {
            kind: kind::REL,
            code: axis::WHEEL,
            minimum: i32::MIN,
            maximum: i32::MAX
        },
    ];
    let pointer = stream::register_hardware(Description {
        capabilities,
        keyboard: None,
    })?;
    DEVICES.call_once(|| Devices { keyboard, pointer });
    ARMED.store(true, Ordering::Release);
    stream::wake_service();
    Ok(())
}

/// The single driver-facing entry: no waiting, allocation, decode or serial
/// output. A busy/full queue leaves sequence unchanged and sends no ACK, so a
/// numbered sender retries. ACKs for accepted and duplicate packets are deferred.
pub fn serial_line(line: &[u8]) {
    if !armed() {
        return;
    }
    let Some(mut receiver) = RECEIVER.try_lock() else {
        return;
    };
    if receiver.ack_count == super::BACKLOG {
        return;
    }
    let Ok(receipt) = receiver
        .channel
        .accept(line, crate::input::pointer::screen().unwrap_or((0, 0)))
    else {
        return;
    };
    if let Some(ack) = receipt.ack {
        let at = (receiver.ack_head + receiver.ack_count) % super::BACKLOG;
        receiver.acks[at] = Some(ack);
        receiver.ack_count += 1;
    }
    super::QUEUED.fetch_add(receipt.added as u64, Ordering::Relaxed);
    drop(receiver);
    stream::notify_host();
}

struct Flight {
    device: Device,
    events: [Event; 32],
    len: usize,
    time_ns: u64,
    keyboard: bool,
}
struct Service {
    keyboard: crate::drivers::hid_boot::report::Keyboard,
    buttons: u8,
    flight: Option<Flight>,
    next_report_ns: u64,
}
static SERVICE: Mutex<Service> = Mutex::new(Service {
    keyboard: crate::drivers::hid_boot::report::Keyboard::new(),
    buttons: 0,
    flight: None,
    next_report_ns: 0,
});

pub(crate) fn drain_one() -> bool {
    let Some(devices) = DEVICES.get() else {
        return false;
    };
    let mut service = SERVICE.lock();
    let Some(mut receiver) = RECEIVER.try_lock() else {
        return false;
    };
    let ack = if receiver.ack_count == 0 {
        None
    } else {
        let at = receiver.ack_head;
        let ack = receiver.acks[at].take();
        receiver.ack_head = (at + 1) % super::BACKLOG;
        receiver.ack_count -= 1;
        ack
    };
    let packet =
        if service.flight.is_none() && crate::window::events::now_ns() >= service.next_report_ns {
            receiver.channel.pop()
        } else {
            None
        };
    drop(receiver);
    // Serial output is deliberately task-only, after releasing the IRQ's lock.
    if let Some(ack) = ack {
        let acknowledged_ns = crate::window::events::now_ns();
        if ack.pointer {
            crate::serial_println!("\nBXPTR_ACK {:016x}", ack.sequence);
        } else {
            crate::serial_println!("\nBXKEY_ACK {:016x}", ack.sequence);
        }
        if let Some(ticket) = ack.ticket {
            // W14: the same task-only ACK is visible to the requesting case on
            // both arches. Keep the ordinary wire ACK byte-for-byte unchanged.
            crate::diag_println!("HOSTINPUT_ACK {} {:016x} ticket={} ns={}",
                if ack.pointer { "BXPTR_ACK" } else { "BXKEY_ACK" },
                ack.sequence, ticket, acknowledged_ns);
        }
    }
    if let Some(packet) = packet {
        let mut flight = Flight {
            device: devices.keyboard,
            events: [Event::default(); 32],
            len: 0,
            time_ns: crate::window::events::now_ns(),
            keyboard: matches!(packet, Packet::Key(_)),
        };
        let mut emit = |kind, code, value| {
            flight.events[flight.len] = Event { kind, code, value };
            flight.len += 1;
        };
        match packet {
            Packet::Key(report) => {
                // Fixed-size reports cannot produce ReportError::Length.
                service
                    .keyboard
                    .report(&report, flight.time_ns, &mut |code, value| {
                        emit(kind::KEY, code, value)
                    })
                    .expect("host boot report length");
            }
            Packet::Pointer(pointer) => {
                flight.device = devices.pointer;
                emit(kind::ABS, axis::X, pointer.x);
                emit(kind::ABS, axis::Y, pointer.y);
                for (bit, code) in [key::BUTTON_LEFT, key::BUTTON_RIGHT, key::BUTTON_MIDDLE]
                    .into_iter()
                    .enumerate()
                {
                    if (service.buttons ^ pointer.buttons) & (1 << bit) != 0 {
                        emit(
                            kind::KEY,
                            code,
                            i32::from(pointer.buttons & (1 << bit) != 0),
                        );
                    }
                }
                service.buttons = pointer.buttons;
                if pointer.wheel != 0 {
                    emit(kind::REL, axis::WHEEL, pointer.wheel);
                }
            }
        }
        emit(kind::SYN, input_abi::syn::REPORT, 0);
        service.flight = Some(flight);
    }
    if let Some(flight) = &service.flight {
        if !stream::drain_hardware(
            flight.device,
            &flight.events[..flight.len],
            flight.time_ns,
            false,
        ) {
            return false;
        }
        if flight.keyboard {
            service.next_report_ns = crate::window::events::now_ns().saturating_add(10_000_000);
        }
        service.flight = None;
        return true;
    }
    ack.is_some()
}
