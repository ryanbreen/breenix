use super::screen;
use crate::gpu::abi::{record as r, Mode};

fn mode() -> Mode { Mode { width: 2, height: 1, scale: 1, reserved: 0 } }
fn record() -> [u8; r::SIZE] {
    let mut rec = [0; r::SIZE];
    for (at, value) in [(r::VERSION_OFF, 2), (r::SIZE_OFF, 1024), (r::BACKEND, 4),
        (r::FLAGS, 1), (r::WIDTH, 2), (r::HEIGHT, 1), (r::TARGET_WIDTH, 2),
        (r::TARGET_HEIGHT, 1), (r::STRIDE, 8)] {
        rec[at..at+4].copy_from_slice(&u32::to_le_bytes(value));
    }
    rec[r::CONTEXT..r::CONTEXT+8].copy_from_slice(&42u64.to_le_bytes());
    rec[r::PRESENT_FENCE..r::PRESENT_FENCE+8].copy_from_slice(&7u64.to_le_bytes());
    rec
}

#[test]
fn observation_requires_case_owned_whole_presented_scanout() {
    let good = record();
    assert!(screen::validate(&good, mode(), 4, 99).is_ok());
    for (at, value) in [(r::VERSION_OFF, 1), (r::SIZE_OFF, 512), (r::BACKEND, 5),
        (r::FLAGS, 0), (r::UNWRITTEN, 1), (r::WIDTH, 1), (r::HEIGHT, 2),
        (r::X, 1), (r::Y, 1), (r::TARGET_WIDTH, 3), (r::TARGET_HEIGHT, 2),
        (r::PRESENT_FENCE, 0), (r::CONTEXT, 99), (r::CONTEXT, 0), (r::TARGET, 1)] {
        let mut rec = good;
        rec[at..at+4].copy_from_slice(&u32::to_le_bytes(value));
        assert!(screen::validate(&rec, mode(), 4, 99).is_err(), "accepted offset {at}={value}");
    }
}

#[test]
fn canonical_digest_and_record_format() {
    let mut pixels = vec![1, 2, 3, 0, 4, 5, 6, 17];
    let hash = screen::canonical_hash(&mut pixels, mode()).unwrap();
    assert_eq!(pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(hash, libpixcheck::sha256(&pixels));
    assert!(screen::canonical_hash(&mut pixels[..4], mode()).is_err());
    let line = screen::format_record("gpu", "draw/one", "end", Ok((hash, mode())));
    let hex: String = hash.iter().map(|v| format!("{v:02x}")).collect();
    assert_eq!(line, format!("SUITE gpu SCREEN draw/one end sha256={hex} mode=2x1\n"));
    assert_eq!(screen::format_record("gpu", "draw/one", "after-present", Err("readback-failed")),
        "SUITE gpu SCREEN draw/one after-present error=readback-failed\n");
}

#[test]
fn named_checkpoints_and_hashed_side_file() {
    for name in ["first", "after-present", "a-b-c"] { assert!(screen::named(name)); }
    for name in ["", "end", "intent", "-a", "a-", "a--b", "A", "a1", "a b", "a\nb"] {
        assert!(!screen::named(name), "accepted {name:?}");
    }
    let line = screen::format_record("breengel", "composition/one", "after-present", Ok(([3; 32], mode())));
    let end = screen::format_record("breengel", "composition/one", "end", Ok(([4; 32], mode())));
    let body = format!("{line}{end}");
    let bytes = screen::pack(&body);
    assert_eq!(&bytes[..32], &libpixcheck::sha256(body.as_bytes()));
    assert_eq!(screen::unpack(&bytes, "breengel", "composition/one").unwrap(), body);
    let mut corrupt = bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(screen::unpack(&corrupt, "breengel", "composition/one").is_err());
    assert!(screen::unpack(&bytes[..31], "breengel", "composition/one").is_err());
    assert!(screen::unpack(&bytes, "gpu", "composition/one").is_err());
    assert!(screen::unpack(&bytes, "breengel", "composition/two").is_err());
    assert!(screen::unpack(&screen::pack(&format!("{line}{line}")), "breengel", "composition/one").is_err());
    assert!(screen::unpack(&screen::pack(&line.replace("after-present", "bad name")), "breengel", "composition/one").is_err());
    assert!(screen::unpack(&screen::pack(&line.replace("mode=2x1", "mode=0x1")), "breengel", "composition/one").is_err());
}
