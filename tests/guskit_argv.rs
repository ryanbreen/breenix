#[path = "../userspace/programs/src/guskit/options.rs"]
mod options;
use options::Options;

fn parse(args: &[&str]) -> Result<Options, &'static str> {
    Options::parse(args.iter().map(|s| s.to_string()))
}

#[test]
fn session_arguments_accept_ports_and_addresses() {
    for (flag, value, expected) in [
        ("--host", "7891", (true, [127, 0, 0, 1], 7891)),
        ("--join", "7891", (false, [127, 0, 0, 1], 7891)),
        ("--host", "0.0.0.0:7890", (true, [0, 0, 0, 0], 7890)),
        ("--join", "10.0.2.2", (false, [10, 0, 2, 2], 7890)),
        ("--join", "10.0.2.2:9000", (false, [10, 0, 2, 2], 9000)),
    ] {
        let session = parse(&[flag, value, "--name", "artist"]).unwrap().session.unwrap();
        assert_eq!((session.0, session.1, session.2), expected);
    }
    for (flag, value) in [("--host", "1.2.3.4"), ("--join", "1.2.3.4:0"), ("--host", "0")] {
        assert!(parse(&[flag, value, "--name", "artist"]).is_err());
    }
}
