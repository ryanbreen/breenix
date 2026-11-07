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
        (r::X, 1), (r::Y, 1), (r::TARGET_WIDTH, 1), (r::TARGET_HEIGHT, 0),
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
    let line = screen::format_record("gpu", "draw/one", "end", Ok((hash, mode(), mode())));
    let hex: String = hash.iter().map(|v| format!("{v:02x}")).collect();
    assert_eq!(line, format!("SUITE gpu SCREEN draw/one end sha256={hex} mode=2x1 binding=2x1\n"));
    assert_eq!(screen::format_record("gpu", "draw/one", "after-present", Err("readback-failed")),
        "SUITE gpu SCREEN draw/one after-present error=readback-failed\n");
}

#[test]
fn named_checkpoints_and_hashed_side_file() {
    for name in ["first", "after-present", "a-b-c"] { assert!(screen::named(name)); }
    for name in ["", "end", "intent", "-a", "a-", "a--b", "A", "a1", "a b", "a\nb"] {
        assert!(!screen::named(name), "accepted {name:?}");
    }
    let line = screen::format_record("breengel", "composition/one", "after-present", Ok(([3; 32], mode(), mode())));
    let end = screen::format_record("breengel", "composition/one", "end", Ok(([4; 32], mode(), mode())));
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
    end.before_drop(|| { assert!(live); Ok(([5; 32], mode(), mode())) });
    live = false;
    assert!(!live);
    assert_eq!(verdict, Err("judged mismatch"));
    assert_eq!(end.result.as_ref().unwrap().as_ref().unwrap().0, [5; 32]);
    // An explicit mid-case teardown may be followed by a new compositor.
    end.before_drop(|| Ok(([6; 32], mode(), mode())));
    assert_eq!(end.result.as_ref().unwrap().as_ref().unwrap().0, [6; 32]);
    let verdict: Result<(), &str> = Ok(());
    end.before_drop(|| Err("readback-failed".into()));
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

#[test]
fn larger_binding_hashes_only_the_visible_mode_and_records_its_dimensions() {
    let mut rec = record();
    rec[r::TARGET_WIDTH..r::TARGET_WIDTH+4].copy_from_slice(&4u32.to_le_bytes());
    rec[r::TARGET_HEIGHT..r::TARGET_HEIGHT+4].copy_from_slice(&2u32.to_le_bytes());
    assert!(screen::validate(&rec, mode(), 4, 99).is_ok());
    let binding = Mode { width: 4, height: 2, scale: 1, reserved: 0 };
    let line = screen::format_record("gpu", "present/takeover-giveback", "end", Ok(([1;32],mode(),binding)));
    assert!(line.ends_with(" mode=2x1 binding=4x2\n"));
    assert!(screen::unpack(&screen::pack(&line), "gpu", "present/takeover-giveback").is_ok());
    let mut visible = vec![1,2,3,0,4,5,6,0];
    let saved = screen::ReadbackReuse::new(&mut visible, &rec, mode(), 4, 99).unwrap();
    assert!(saved.matches(&rec));
    rec[r::X..r::X+4].copy_from_slice(&1u32.to_le_bytes());
    assert!(screen::validate(&rec, mode(), 4, 99).is_err(), "unknown scan offset");
    assert!(!saved.matches(&rec), "reuse must pin the scan offset");
}

#[test]
fn transient_unknown_binding_must_reach_the_pinned_observer() {
    assert!(!screen::non_case_binding(false, 0, 10, 20));
    assert!(!screen::non_case_binding(false, 30, 10, 20));
    assert!(!screen::non_case_binding(false, 10, 10, 20));
    assert!(!screen::non_case_binding(false, 20, 10, 20));
    assert!(screen::non_case_binding(true, 0, 10, 20), "confirmed empty binding after reset");
    assert!(!screen::non_case_binding(true, 30, 10, 20));
    assert!(screen::non_case_binding(true, 10, 10, 20));
    assert!(screen::non_case_binding(true, 20, 10, 20));
}

#[test]
fn checkpoint_watchdog_and_window_baseline_are_independent_of_paused_time() {
    let clock = screen::CheckpointClock::new();
    clock.start(100);
    clock.stop(140);
    let baseline = 40;
    assert_eq!(clock.elapsed_from(200, 230, baseline), 30);
    clock.start(230);
    assert_eq!(clock.elapsed_from(200, 260, baseline), 30);
    assert!(!clock.overdue(230 + 119_999_999_999));
    assert!(clock.overdue(230 + 120_000_000_000));
    clock.stop(270);
    assert_eq!(clock.elapsed_from(200, 300, baseline), 60);
}

#[test]
fn native_shared_channel_does_not_identify_the_runner_surface() {
    let mut rec = record();
    rec[r::BACKEND..r::BACKEND+4].copy_from_slice(&6u32.to_le_bytes());
    rec[r::CONTEXT..r::CONTEXT+8].copy_from_slice(&99u64.to_le_bytes());
    rec[r::RESOURCE..r::RESOURCE+8].copy_from_slice(&30u64.to_le_bytes());
    assert!(!screen::non_case_binding(true, 30, 10, 20));
    assert!(screen::validate(&rec, mode(), 6, 99).is_ok(), "native channel is shared by distinct surfaces");
    assert!(screen::non_case_binding(true, 20, 10, 20), "runner resource still excluded");
    rec[r::CONTEXT..r::CONTEXT+8].fill(0);
    assert!(screen::validate(&rec, mode(), 6, 99).is_err(), "missing native identity still refused");
}

#[test]
fn endpoint_errors_keep_exact_causes_and_only_known_end_states_are_declared() {
    use crate::gpu::abi::screen::Failure;
    for (reason, failed, declared) in [(4, 1234, true), (4, 0, false), (5, 0, false),
        (6, 0, false), (7, 0, false), (8, 0, true), (10, 0, false)] {
        let failure = Failure { errno: 5, reason, failed_submission: failed, declared: if declared { 1 } else { 0 } };
        let error = screen::endpoint_error(5, &failure.encode());
        assert_eq!(error.starts_with("unobservable reason="), declared);
        assert!(error.contains(&format!("errno=5 eio_reason={reason} failed_submission={failed}")));
        let line = screen::format_record("gpu", "draw/one", "end", Err(&error));
        assert!(screen::unpack(&screen::pack(&line), "gpu", "draw/one").is_ok());
    }
    let error = screen::endpoint_error(110, &[0; 32]);
    assert_eq!(error, "readback-failed errno=110 eio_reason=0 failed_submission=0");
}

#[test]
fn unexpected_target_invalidation_cannot_be_declared() {
    let failure = crate::gpu::abi::screen::Failure { errno: 5, reason: 4,
        failed_submission: 1234, declared: 0 };
    let error = screen::endpoint_error(5, &failure.encode());
    assert!(error.starts_with("readback-failed "));
}

#[test]
fn crc_screen_record_and_diagnostic_label() {
    let binding = gpu_abi::screen_crc::Binding {resource: 7, present_fence: 9};
    let record = gpu_abi::screen_crc::Record {
        suite:"gpu".into(), case:"draw/one".into(), checkpoint:"end".into(),
        binding_before:binding, binding_after:binding, elapsed_us:1000,
        disarmed:true, display_error:0, status:1,
        output_config:gpu_abi::screen_crc::OutputConfig {mode:[2,1],raster:[1280,1024],
            viewport:[0,112,1280,800],sor:0,rg:0,methods:vec![(0x200,1)]},
        samples:vec![gpu_abi::screen_crc::Sample {status:1,crc:0x1234,crc2:0xabcd};5],
    };
    let bytes = record.encode().unwrap();
    let line = screen::format_crc_record("gpu", "draw/one", "end", [3;32], mode(), mode(), &bytes).unwrap();
    assert_eq!(line, format!("SUITE gpu SCREEN draw/one end sha256={} mode=2x1 binding=2x1 crc=00001234 crc2=0000abcd raster=1280x1024 out=sor0\n", "03".repeat(32)));
    assert!(screen::unpack(&screen::pack(&line),"gpu","draw/one").is_ok());
    assert!(screen::format_crc_record("gpu", "draw/two", "end", [3;32], mode(), mode(), &bytes).is_err());
    let label = screen::crc_label("gpu", "draw/one", "end");
    let hash = libpixcheck::sha256(b"draw/one\0end");
    let short: String = hash[..8].iter().map(|b|format!("{b:02x}")).collect();
    assert_eq!(label, format!("crc-gpu-{short}"));
    assert!(label.len() <= 64 && label.bytes().all(|b|b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
    assert_ne!(label, screen::crc_label("gpu", "draw/one", "after-present"));
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/screen-verdict.py");
    let python = "import importlib.util,re,sys; s=importlib.util.spec_from_file_location('v',sys.argv[1]); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); assert m.RECORD.fullmatch(sys.argv[2].strip()); assert re.fullmatch(r'[a-z][a-z0-9-]{0,63}',sys.argv[3])";
    assert!(std::process::Command::new("python3").args(["-c",python]).arg(script).arg(&line).arg(label).status().unwrap().success());
}
