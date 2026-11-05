#[path = "../userspace/programs/src/bwm/clipboard.rs"]
mod clipboard;

use clipboard::{Decoder, Request, Store, MAX_PAYLOAD};

fn wire(op: u32, window: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = b"BCLIP001".to_vec();
    out.extend(op.to_le_bytes());
    out.extend((payload.len() as u32).to_le_bytes());
    out.extend(window.to_le_bytes());
    out.extend(payload);
    out
}

fn parse(bytes: &[u8], chunk: usize) -> Result<Request, i32> {
    let mut decoder = Decoder::new();
    for part in bytes.chunks(chunk) {
        let (used, result) = decoder.feed(part);
        if let Some(result) = result { assert_eq!(used, part.len()); return result; }
        assert_eq!(used, part.len());
    }
    panic!("incomplete request")
}

fn exchange(store: &mut Store, op: u32, window: u64, data: &[u8], chunk: usize) -> Vec<u8> {
    store.execute(parse(&wire(op, window, data), chunk).unwrap()).encode()
}
fn fields(reply: &[u8]) -> (i32, u32, u64, &[u8]) {
    (i32::from_le_bytes(reply[0..4].try_into().unwrap()), u32::from_le_bytes(reply[4..8].try_into().unwrap()), u64::from_le_bytes(reply[8..16].try_into().unwrap()), &reply[16..])
}

#[test]
fn suite_payloads_and_replacement_survive_chunking_and_window_changes() {
    for chunk in [1, 65536] {
        let mut store = Store::new();
        let initial = exchange(&mut store, 2, 9, &[], chunk);
        let (status, len, mut serial, data) = fields(&initial);
        assert_eq!((status, len, data), (0, 0, &[][..]));
        assert_ne!(serial, 0);
        for payload in [b"from the first program".to_vec(), "héλ🙂\n".as_bytes().to_vec(), vec![b'x'; 1024 * 1024], b"one\0two\0three".to_vec(), b"first".to_vec(), b"second".to_vec(), b"retained".to_vec(), vec![]] {
            let set = exchange(&mut store, 1, 11, &payload, chunk);
            let (status, len, next, data) = fields(&set);
            assert_eq!((status, len, data), (0, 0, &[][..]));
            assert!(next > serial); serial = next;
            let get = exchange(&mut store, 2, 22, &[], chunk);
            let (status, len, got, data) = fields(&get);
            assert_eq!((status, len as usize, got, data), (0, payload.len(), serial, payload.as_slice()));
        }
    }
}

#[test]
fn bound_is_inclusive_and_refusals_preserve_store() {
    let mut store = Store::new();
    for size in [MAX_PAYLOAD - 1, MAX_PAYLOAD] {
        let payload = vec![0x5a; size];
        let reply = exchange(&mut store, 1, 1, &payload, 65536);
        assert_eq!(fields(&reply).0, 0);
        let got = exchange(&mut store, 2, 2, &[], 65536);
        assert_eq!(fields(&got).3, payload);
    }
    let before = exchange(&mut store, 2, 2, &[], 65536);
    let mut header = wire(1, 1, &[]); header[12..16].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_le_bytes());
    assert_eq!(parse(&header, 1).unwrap_err(), -7);
    assert_eq!(exchange(&mut store, 2, 2, &[], 65536), before);
}

#[test]
fn malformed_headers_are_refused() {
    let mut bad_magic = wire(1, 1, &[]); bad_magic[0] = b'X';
    let bad_op = wire(3, 1, &[]);
    let get_payload = wire(2, 1, b"x");
    for wire in [&bad_magic[..], &bad_op[..], &get_payload[..24]] {
        for chunk in [1, 65536] { assert_eq!(parse(wire, chunk).unwrap_err(), -22); }
    }
}
