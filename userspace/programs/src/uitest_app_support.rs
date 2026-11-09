use std::fmt::Write;

pub struct Options<'a> {
    pub settle_control: bool,
    pub drain_no_render: bool,
    pub title: &'a str,
    pub crash: Option<i32>,
    pub hang: bool,
    pub stderr_flood: usize,
    pub session_probe: bool,
    pub config_probe: bool,
    pub canonical: bool,
    pub capture_control: bool,
    pub widgets: bool,
    pub pointer_probe: bool,
    pub two_windows: bool,
    pub clock_probe: bool,
    pub timer: bool,
    pub animate: bool,
    pub real_clock: bool,
}

pub fn parse<'a>(args: &[&'a str]) -> Result<Options<'a>, &'static str> {
    let title = *args.first().ok_or("missing window title")?;
    if title.is_empty() || title.starts_with("--") { return Err("invalid window title"); }
    let mut options = Options { settle_control: false, drain_no_render: false, title, crash: None, hang: false, stderr_flood: 0, session_probe: false, config_probe: false, canonical: false, capture_control: false, widgets: false, pointer_probe: false, two_windows: false, clock_probe: false, timer: false, animate: false, real_clock: false };
    let mut flood_seen = false;
    for arg in &args[1..] {
        if *arg == "--settle-control" && !options.settle_control {
            options.settle_control = true;
        } else if *arg == "--drain-no-render" && !options.drain_no_render {
            options.drain_no_render = true;
        } else if *arg == "--timer" && !options.timer {
            options.timer = true;
        } else if *arg == "--animate" && !options.animate {
            options.animate = true;
        } else if *arg == "--real-clock" && !options.real_clock {
            options.real_clock = true;
        } else if *arg == "--clock-probe" && !options.clock_probe {
            options.clock_probe = true;
        } else if *arg == "--two-windows" && !options.two_windows {
            options.two_windows = true;
        } else if *arg == "--pointer-probe" && !options.widgets {
            options.widgets=true;options.pointer_probe=true;
        } else if *arg == "--widgets" && !options.widgets {
            options.widgets = true;
        } else if *arg == "--capture-control" && !options.capture_control {
            options.capture_control = true;
        } else if *arg == "--canonical" && !options.canonical {
            options.canonical = true;
        } else if *arg == "--hang" && !options.hang {
            options.hang = true;
        } else if *arg == "--session-probe" && !options.session_probe {
            options.session_probe = true;
        } else if *arg == "--config-probe" && !options.config_probe {
            options.config_probe = true;
        } else if let Some(value) = arg.strip_prefix("--crash=") {
            if options.crash.is_some() { return Err("duplicate crash flag"); }
            let signal: i32 = value.parse().map_err(|_| "invalid signal")?;
            if !(1..=31).contains(&signal) { return Err("invalid signal"); }
            options.crash = Some(signal);
        } else if let Some(value) = arg.strip_prefix("--stderr-flood=") {
            if flood_seen { return Err("duplicate flood flag"); }
            options.stderr_flood = value.parse().map_err(|_| "invalid flood size")?;
            flood_seen = true;
        } else { return Err("unknown or duplicate flag"); }
    }
    if options.drain_no_render && !options.settle_control {
        return Err("drain without render requires settle control");
    }
    if options.settle_control && (options.timer || options.animate || options.real_clock || options.clock_probe || options.widgets || options.capture_control || options.canonical || options.hang || options.crash.is_some() || options.session_probe || options.config_probe || flood_seen || options.two_windows) {
        return Err("settle control cannot be combined with other app modes");
    }
    if options.real_clock && !options.timer && !options.animate {
        return Err("real clock requires timer or animation mode");
    }
    if (options.timer || options.animate) && (options.capture_control || options.widgets || options.clock_probe || options.canonical || options.hang || options.crash.is_some() || options.session_probe || options.config_probe || flood_seen || options.two_windows) {
        return Err("timer mode cannot be combined with other app modes");
    }
    if options.capture_control && (options.clock_probe || options.widgets || options.canonical || options.hang || options.crash.is_some() || options.session_probe || options.config_probe || flood_seen) {
        return Err("capture control cannot be combined with other modes");
    }
    if options.widgets && (options.clock_probe || options.canonical || options.hang || options.crash.is_some() || options.session_probe || options.config_probe || flood_seen) {
        return Err("widgets cannot be combined with other modes");
    }
    if options.two_windows && (!options.widgets || options.pointer_probe) {
        return Err("two windows requires plain widget mode");
    }
    Ok(options)
}

// Counts and hexadecimal bytes preserve empty entries, arbitrary bytes and boundaries.
pub fn echo(args: &[&[u8]], env: &[&[u8]]) -> String {
    let mut line = format!("UITEST-ECHO {}", args.len());
    for (index, group) in [args, env].into_iter().enumerate() {
        if index == 1 { write!(line, " {}", env.len()).unwrap(); }
        for entry in group {
            line.push(' ');
            for byte in *entry { write!(line, "{byte:02x}").unwrap(); }
        }
    }
    line
}
