//! Host checks of the inner executor's production wire format and classifier.
//! Run: rustc --edition=2021 -Dwarnings --test tests/uitest_outcome.rs
#[path = "../libs/libuitest/src/outcome.rs"]
mod outcome;
use outcome::*;

fn fault(class: FailureClass, operation: &str, evidence: &str) -> ScenarioOutcome {
    ScenarioOutcome::fail(class, operation, evidence)
}

#[test]
fn outcome_codec_round_trip_and_frame_refusals() {
    let mut cases = vec![ScenarioOutcome::pass()];
    for class in FailureClass::ALL {
        cases.push(fault(class, "expect_bounds(submit)", "node=submit bounds=12,4,30,20 reported=13,4,30,20 λ"));
    }
    for mut value in cases {
        value.records_mut().extend(["FRAME inner/one label".into(), "TREE inner/one button".into()]);
        let frame = encode(&value).unwrap();
        assert_eq!(decode(&frame), Ok(value));
        for end in 0..frame.len() {
            assert!(decode(&frame[..end]).is_err(), "accepted truncated frame at {end}");
        }
        let mut extra = frame.clone(); extra.push(0);
        assert!(decode(&extra).is_err(), "accepted trailing bytes");
        let mut oversized = frame.clone();
        oversized[8..12].copy_from_slice(&((MAX_OUTCOME_BYTES + 1) as u32).to_le_bytes());
        assert_eq!(decode(&oversized), Err(CodecError::Oversized));
        let mut magic = frame.clone(); magic[0] ^= 1;
        assert!(decode(&magic).is_err());
        let mut tag = frame.clone(); tag[12] = 255;
        assert!(decode(&tag).is_err());
    }
    let mut max = fault(FailureClass::TreeMismatch, "query", &"x".repeat(MAX_OUTCOME_BYTES));
    assert_eq!(encode(&max), Err(CodecError::Oversized));
    max = ScenarioOutcome::pass();
    max.records_mut().resize(257, "x".into());
    assert!(encode(&max).is_err());
    let mut record = ScenarioOutcome::pass();
    record.records_mut().push("x".repeat(4096));
    assert!(encode(&record).is_err());
    let mut invalid_utf8 = encode(&fault(FailureClass::TreeMismatch, "op", "evidence")).unwrap();
    *invalid_utf8.last_mut().unwrap() = 255;
    assert!(decode(&invalid_utf8).is_err());
    // The bound includes the header and length fields, not just evidence bytes.
    let full = fault(FailureClass::PixelMismatch, "x", &"a".repeat(MAX_OUTCOME_BYTES - 24));
    let frame = encode(&full).unwrap();
    assert_eq!(frame.len(), MAX_OUTCOME_BYTES);
    assert_eq!(decode(&frame), Ok(full));
}

#[test]
fn classification_requires_exact_fault_operation_and_evidence() {
    use Classification::*;
    let pixel = Expected::Fault { class: FailureClass::PixelMismatch, operation: "label", evidence: "first=12,4" };
    let rows = [
        (ScenarioOutcome::pass(), UnexpectedPass),
        (fault(FailureClass::PixelMismatch, "label", "first=12,4"), ExpectedFault),
        (fault(FailureClass::TreeMismatch, "label", "first=12,4"), WrongClass),
        (fault(FailureClass::PixelMismatch, "other", "first=12,4"), WrongOperation),
        (fault(FailureClass::PixelMismatch, "label", "first=13,4"), WrongEvidence),
        (fault(FailureClass::Infrastructure, "label", "first=12,4"), InfrastructureError),
        (fault(FailureClass::AppCrashed, "label", "first=12,4"), Crash),
        (fault(FailureClass::KernelFault, "label", "first=12,4"), KernelFault),
    ];
    for (actual, want) in rows {
        assert_eq!(classify(&pixel, &actual), want);
        assert_eq!(want.accepted(), want == ExpectedFault);
    }
    assert_eq!(classify(&Expected::Pass, &ScenarioOutcome::pass()), CleanPass);
    for class in FailureClass::ALL {
        let actual = fault(class, "op", "evidence");
        let expected = Expected::Fault { class, operation: "op", evidence: "evidence" };
        let result = classify(&expected, &actual);
        assert_eq!(result.accepted(), !matches!(class, FailureClass::Infrastructure | FailureClass::KernelFault));
        assert!(!classify(&Expected::Pass, &actual).accepted());
    }
    assert!(CleanPass.accepted());
    assert!(!UnexpectedFailure.accepted());
}
