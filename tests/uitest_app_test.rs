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
