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

#[test]
fn checkpoint_clock_excludes_only_its_own_measured_interval() {
    let clock = screen::CheckpointClock::new();
    assert_eq!(clock.elapsed(100, 150), 50);
    clock.start(150);
    assert_eq!(clock.elapsed(100, 180), 50, "parent timeout must pause during observation");
    clock.stop(190);
    assert_eq!(clock.elapsed(100, 200), 60, "helper cleanup still belongs to the case");
    clock.start(210);
    clock.stop(217);
    assert_eq!(clock.elapsed(100, 230), 83);
}

#[test]
fn end_is_observed_before_teardown_and_errors_are_separate_from_verdict() {
    let mut end = screen::EndCheckpoint::default();
    let mut live = true;
    let verdict: Result<(), &str> = Err("judged mismatch");
    end.before_drop(|| { assert!(live); Ok(([5; 32], mode())) });
    live = false;
    assert!(!live);
    assert_eq!(verdict, Err("judged mismatch"));
    assert_eq!(end.result.as_ref().unwrap().as_ref().unwrap().0, [5; 32]);
    // An explicit mid-case teardown may be followed by a new compositor.
    end.before_drop(|| Ok(([6; 32], mode())));
    assert_eq!(end.result.as_ref().unwrap().as_ref().unwrap().0, [6; 32]);
    let verdict: Result<(), &str> = Ok(());
    end.before_drop(|| Err("readback-failed"));
    assert_eq!(verdict, Ok(()));
    assert_eq!(end.result.unwrap().unwrap_err(), "readback-failed");
}

#[test]
fn runtime_switch_defaults_on_and_disables_all_checkpoint_work() {
    for option in ["", "on", "on\n"] {
        assert!(screen::configured("gpu", option));
        assert!(screen::configured("breengel", option));
    }
    assert!(!screen::configured("gpu", "off\n"));
    assert!(!screen::configured("breengel", "off"));
    assert!(!screen::configured("time", "on"));
}

#[test]
fn reuse_requires_the_same_whole_scanout_and_present_fence() {
    let rec = record();
    let mut pixels = vec![1,2,3,0,4,5,6,0];
    let saved = screen::ReadbackReuse::new(&mut pixels, &rec, mode(), 4, 99).unwrap();
    assert!(saved.matches(&rec));
    for at in [r::RESOURCE, r::PRESENT_FENCE, r::CONTEXT, r::WIDTH, r::BACKEND] {
        let mut changed = rec;
        changed[at] ^= 1;
        assert!(!saved.matches(&changed), "reused changed binding at {at}");
    }
    assert_eq!(saved.hash, libpixcheck::sha256(&[1,2,3,255,4,5,6,255]));
    assert!(screen::ReadbackReuse::new(&mut pixels[..4], &rec, mode(), 4, 99).is_err());
}
