//! The host channel's real wire parser and bounded admission queue.
#[path = "../kernel/src/input/host.rs"]
pub mod host;

#[test]
fn pointer_vectors_round_trip_and_refuse_invalid_input() {
    for line in include_str!("fixtures/bxptr-vectors.txt").lines().filter(|l| !l.starts_with('#')) {
        let (fields, wire) = line.split_once(" | ").unwrap();
        let f: Vec<_> = fields.split_whitespace().collect();
        let n = |i: usize| f[i].parse::<u32>().unwrap();
        let screen = (n(0), n(1));
        let mut channel = host::Channel::new();
        // Establish the preceding sequence through the same admission path.
        for seq in 1..=n(2) {
            let packet = pointer_line(seq as u64, "0 0 2560 1600 0 0");
            channel.accept(packet.as_bytes(), screen).unwrap();
            channel.pop().unwrap();
        }
        let result = channel.accept(wire.as_bytes(), screen);
        match f[5] {
            "accept" => {
                assert_eq!(result.unwrap().added, 1, "{wire}");
                let host::Packet::Pointer(p) = channel.pop().unwrap() else { panic!("pointer expected") };
                assert_eq!(p.x as u64 * (screen.0 - 1) as u64 / 32767, n(3) as u64);
                assert_eq!(p.y as u64 * (screen.1 - 1) as u64 / 32767, n(4) as u64);
                assert_eq!((p.buttons, p.wheel), (5, -1));
            }
            "repeat" => {
                assert_eq!(result.unwrap().added, 0, "a retransmission must not publish twice");
                assert!(channel.pop().is_none());
            }
            _ => { assert!(result.is_err(), "{wire}"); assert!(channel.pop().is_none()); }
        }
    }
    // Check each pixel at widths 2, 1280, 1600, 2560 and 32768.
    for width in [2, 1280, 1600, 2560, 32768] {
        for x in 0..width {
            let v = host::absolute_value(x, width).unwrap();
            assert_eq!(v as u64 * (width - 1) as u64 / 32767, x as u64);
        }
    }
    assert!(host::absolute_value(0, 32769).is_err());
    assert!(host::absolute_value(2560, 2560).is_err());
}

fn pointer_line(seq: u64, payload: &str) -> String {
    let body = format!("{seq:016x} {payload}");
    let mut crc = !0u32;
    for b in body.bytes() {
        crc ^= b as u32;
        for _ in 0..8 { crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1)); }
    }
    format!("BXPTR @{seq:016x}:{:08x} {payload}", !crc)
}

#[test]
fn bxkey_admission_is_atomic_and_retransmissions_ack_without_replaying() {
    let wire = b"BXKEY @0000000000000001:8cdc16e0 0000040000000000,0000000000000000";
    let mut channel = host::Channel::new();
    let receipt = channel.accept(wire, (2560, 1600)).unwrap();
    assert_eq!(receipt.added, 2);
    assert_eq!(receipt.ack.unwrap().sequence, 1);
    let receipt = channel.accept(wire, (2560, 1600)).unwrap();
    assert_eq!(receipt.added, 0);
    assert_eq!(receipt.ack.unwrap().sequence, 1);
    assert!(matches!(channel.pop(), Some(host::Packet::Key([0,0,4,0,0,0,0,0]))));
    assert!(matches!(channel.pop(), Some(host::Packet::Key(r)) if r == [0; 8]));
    assert!(channel.pop().is_none());
    // Whole-packet refusal must leave sequence 1 available to retry.
    let mut full = host::Channel::new();
    for _ in 0..host::BACKLOG - 1 { full.accept(b"BXKEY 0000000000000000", (2560,1600)).unwrap(); }
    assert!(full.accept(wire, (2560,1600)).is_err());
    full.pop();
    assert_eq!(full.accept(wire, (2560,1600)).unwrap().added, 2);
}

#[test]
fn serial_transports_use_the_shared_irq_safe_entry() {
    let host = include_str!("../kernel/src/input/host/runtime.rs");
    let serial = host.split("pub fn serial_line(").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert!(serial.contains("try_lock()"));
    for forbidden in [".lock()", "serial_print", "log::", "register_hardware", "publish_hardware"] {
        assert!(!serial.contains(forbidden), "IRQ serial entry contains {forbidden}");
    }
    for source in [include_str!("../kernel/src/serial/command.rs"), include_str!("../kernel/src/serial_aarch64.rs")] {
        assert!(source.contains("host::serial_line("));
    }
    assert!(host.contains("hid_boot::report::Keyboard"));
    assert!(!host.contains("target_arch"));
}

#[test]
fn only_host_channel_declarations_arm_serial_input() {
    for value in ["host-channel", "host-channel;missing=keyboard"] {
        assert!(host::declares_channel(Some(value)), "{value}");
    }
    for value in ["qmp", "prlsdk", "vnc", "", "host-channel-extra"] {
        assert!(!host::declares_channel(Some(value)), "{value}");
    }
    assert!(!host::declares_channel(None));
}
