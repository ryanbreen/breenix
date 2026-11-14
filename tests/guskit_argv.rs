#[path = "../userspace/programs/src/guskit/dimensions.rs"]
mod dimensions;
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

#[test]
fn canvas_limits_follow_the_largest_window_content_and_startup_uses_the_work_area() {
    let default=parse(&[]).unwrap();
    assert!(!default.explicit_canvas && default.file.is_none() && default.session.is_none());
    assert_eq!((default.width,default.height),(800,424));
    for scale in [1,2] {
        let (w,h)=dimensions::initial_window(2560,1600,scale);
        assert!(w/scale>=800 && h/scale>=600);
        assert!(w>2560*8/10 && h>1600*7/10,"most of the work area");
    }
    let largest=format!("{}x{}",dimensions::MAX_WIDTH,dimensions::MAX_HEIGHT);
    let parsed=parse(&["--canvas",&largest]).unwrap();
    assert!(parsed.explicit_canvas);
    assert_eq!((parsed.width,parsed.height),(4096,3920));
    assert!(parse(&["--canvas","4097x3920"]).is_err());
    assert!(parse(&["--canvas","4096x3921"]).is_err());
}
