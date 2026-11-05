use libbreenix::input::{self, abi, Batch, Capability, Event, Records};
use std::mem::{offset_of, size_of};

#[test]
fn input_wire_layout_and_creation_encoding() {
    assert_eq!(size_of::<Event>(), 32);
    assert_eq!(
        [
            offset_of!(Event, serial),
            offset_of!(Event, time_ns),
            offset_of!(Event, device),
            offset_of!(Event, kind),
            offset_of!(Event, code),
            offset_of!(Event, value),
            offset_of!(Event, flags)
        ],
        [0, 8, 16, 20, 22, 24, 28]
    );
    assert_eq!(size_of::<abi::CreateHeader>(), 16);
    assert_eq!(
        [
            offset_of!(abi::CreateHeader, magic),
            offset_of!(abi::CreateHeader, flags),
            offset_of!(abi::CreateHeader, capabilities)
        ],
        [0, 8, 12]
    );
    assert_eq!(size_of::<Capability>(), 12);
    assert_eq!(
        [
            offset_of!(Capability, kind),
            offset_of!(Capability, code),
            offset_of!(Capability, minimum),
            offset_of!(Capability, maximum)
        ],
        [0, 2, 4, 8]
    );
    assert_eq!(abi::TIME_INJECTOR, 1);
    let mut bytes = [0u8; 28];
    let cap = Capability {
        kind: 3,
        code: 1,
        minimum: -2,
        maximum: 32767,
    };
    assert_eq!(input::encode_create(&mut bytes, &[cap], true).unwrap(), 28);
    assert_eq!(
        bytes,
        [
            b'B', b'I', b'N', b'P', b'U', b'T', b'0', b'1', 1, 0, 0, 0, 1, 0, 0, 0, 3, 0, 1, 0,
            254, 255, 255, 255, 255, 127, 0, 0
        ]
    );
}

#[test]
fn text_batch_round_trips_wire_records() {
    let mut storage = [0u8; 128];
    let mut batch = Batch::new(&mut storage);
    batch
        .text("hé", 0x0102030405060708)
        .unwrap()
        .finish(0x0102030405060709)
        .unwrap();
    let bytes = batch.bytes();
    assert_eq!(&bytes[8..16], &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(&bytes[20..28], &[6, 0, 0, 0, 3, 0, 0, 0]);
    assert_eq!(&bytes[32..35], "hé".as_bytes());
    let mut records = Records::from_bytes(bytes);
    let text = records.next().unwrap().unwrap();
    assert_eq!(
        text.event,
        Event {
            time_ns: 0x0102030405060708,
            kind: 6,
            value: 3,
            ..Event::default()
        }
    );
    assert_eq!(text.payload, "hé".as_bytes());
    let syn = records.next().unwrap().unwrap();
    assert_eq!(
        syn.event,
        Event {
            time_ns: 0x0102030405060709,
            ..Event::default()
        }
    );
    assert!(records.next().is_none());
}
