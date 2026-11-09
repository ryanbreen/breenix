#[path = "../userspace/programs/src/uitest_app_support.rs"]
mod support;

#[test]
fn echo_round_trips_bytes_and_boundaries() {
    let args: Vec<&[u8]> = vec![b"app", b"", b"a b\n\\\"", b"\xff"];
    let env: Vec<&[u8]> = vec![b"A=x y", b"B=\n=\\"];
    let line = support::echo(&args, &env);
    assert!(!line.contains('\n'));
    let mut parts = line.split(' ');
    assert_eq!(parts.next(), Some("UITEST-ECHO"));
    assert_eq!(parts.next(), Some("4"));
    let decode = |s: &str| -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        s.as_bytes().chunks_exact(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap()).collect()
    };
    for arg in &args { assert_eq!(decode(parts.next().unwrap()), *arg); }
    assert_eq!(parts.next(), Some("2"));
    for entry in &env { assert_eq!(decode(parts.next().unwrap()), *entry); }
    assert_eq!(parts.next(), None);
}

#[test]
fn flags_are_checked() {
    let parsed = support::parse(&["window", "--crash=11", "--hang", "--stderr-flood=65537"]).unwrap();
    assert_eq!(parsed.title, "window");
    let probe = support::parse(&["window", "--pointer-probe"]).unwrap();
    assert!(probe.widgets && probe.pointer_probe);
    assert_eq!(parsed.crash, Some(11));
    assert!(parsed.hang);
    assert_eq!(parsed.stderr_flood, 65537);
    for args in [vec!["window", "--bogus"], vec!["window", "--crash=0"], vec!["window", "--stderr-flood=no"], vec!["window", "--hang", "--hang"]] {
        assert!(support::parse(&args).is_err(), "{args:?}");
    }
}

#[test]
fn two_widget_windows_use_one_app_mode() {
    let options=support::parse(&["pair", "--widgets", "--two-windows"]).unwrap();
    assert!(options.widgets && options.two_windows);
    assert!(support::parse(&["pair", "--two-windows"]).is_err());
    assert!(support::parse(&["pair", "--widgets", "--two-windows", "--two-windows"]).is_err());
}

#[test]
fn timer_modes_and_deliberate_real_clock_read_are_explicit() {
    let timer = support::parse(&["timer", "--timer"]).unwrap();
    assert!(timer.timer && !timer.animate && !timer.real_clock);
    let animation = support::parse(&["animation", "--animate", "--real-clock"]).unwrap();
    assert!(animation.animate && animation.real_clock);
    for flags in [vec!["app", "--real-clock"], vec!["app", "--animate", "--widgets"],
        vec!["app", "--animate", "--animate"], vec!["app", "--timer", "--capture-control"]] {
        assert!(support::parse(&flags).is_err(), "{flags:?}");
    }
}

#[test]
fn settle_controls_and_drain_without_commit_are_explicit() {
    let controlled = support::parse(&["app", "--settle-control"]).unwrap();
    assert!(controlled.settle_control && !controlled.drain_no_render);
    let drain = support::parse(&["app", "--settle-control", "--drain-no-render"]).unwrap();
    assert!(drain.settle_control && drain.drain_no_render);
    for flags in [vec!["app", "--drain-no-render"],
        vec!["app", "--settle-control", "--settle-control"],
        vec!["app", "--settle-control", "--widgets"],
        vec!["app", "--settle-control", "--timer"],
        vec!["app", "--settle-control", "--hang"]] {
        assert!(support::parse(&flags).is_err(), "{flags:?}");
    }
}
