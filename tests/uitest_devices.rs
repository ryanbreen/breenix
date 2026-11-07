#[path = "../libs/libuitest/src/supervisor.rs"]
pub mod supervisor;
use supervisor::devices::{InputState, RawInput};

#[test]
fn release_set_tracks_successful_edges_and_retries_failed_releases() {
    let mut state = InputState::default();
    state.regrab(7, || Ok(())).unwrap();
    let mut writes = Vec::new();
    for (code, value) in [(42, 1), (30, 1), (30, 1), (272, 1), (273, 1), (30, 0)] {
        assert!(state.send(RawInput { kind: 1, code, value }, Some(7), |e| { writes.push(e); Ok(()) }).unwrap());
    }
    assert!(state.send(RawInput { kind: 2, code: 8, value: -2 }, Some(7), |_| Ok(())).unwrap());
    assert!(state.send(RawInput { kind: 1, code: 274, value: 1 }, Some(7), |_| Err("write failed".into())).is_err());
    state.pause();
    let mut releases = Vec::new();
    assert!(state.release_all(|e| {
        releases.push(e);
        if e.code == 272 { Err("retry".into()) } else { Ok(()) }
    }).is_err());
    assert_eq!(releases, [42, 272, 273].map(|code| RawInput { kind: 1, code, value: 0 }));
    releases.clear();
    state.release_all(|e| { releases.push(e); Ok(()) }).unwrap();
    assert_eq!(releases, [RawInput { kind: 1, code: 272, value: 0 }]);
    state.release_all(|_| panic!("already released")).unwrap();
}

#[test]
fn injection_waits_for_the_successful_regrab_and_matching_owner() {
    let mut state = InputState::default();
    let key = RawInput { kind: 1, code: 30, value: 1 };
    assert!(!state.send(key, Some(7), |_| panic!("no startup grab")).unwrap());
    state.regrab(7, || Ok(())).unwrap();
    assert!(state.send(key, Some(7), |_| Ok(())).unwrap());
    // A death observation closes the gate before claim recovery begins.
    state.pause();
    assert!(!state.send(key, Some(8), |_| panic!("claim alone is insufficient")).unwrap());
    assert!(state.regrab(8, || Err("busy".into())).is_err());
    assert!(!state.send(key, Some(8), |_| panic!("failed grab")).unwrap());
    state.regrab(8, || Ok(())).unwrap();
    assert!(state.send(key, Some(8), |_| Ok(())).unwrap());
    assert!(!state.send(key, None, |_| panic!("compositor died before reap")).unwrap());
    assert!(!state.send(key, Some(8), |_| panic!("old witness survived loss")).unwrap());
    state.regrab(8, || Ok(())).unwrap();
    assert!(!state.send(key, Some(9), |_| panic!("different owner")).unwrap());
}

#[test]
fn raw_input_wire_preserves_edges_motion_wheel_and_device_identity() {
    use supervisor::{Message, encode, decode};
    for message in [Message::Devices([1024, 1025]), Message::Input(RawInput { kind: 1, code: 42, value: 0 }),
        Message::Input(RawInput { kind: 2, code: 0, value: -4096 }), Message::InputDone] {
        let wire = encode(&message).unwrap();
        assert_eq!(decode(&wire[4..]).unwrap(), message);
    }
    for event in [RawInput { kind: 3, code: 0, value: 0 }, RawInput { kind: 1, code: 30, value: 2 },
        RawInput { kind: 1, code: 0, value: 1 }, RawInput { kind: 2, code: 7, value: 1 }] {
        assert!(encode(&Message::Input(event)).is_err());
    }
    assert!(encode(&Message::Devices([4, 4])).is_err());
    assert!(encode(&Message::Devices([0, 4])).is_err());
}
