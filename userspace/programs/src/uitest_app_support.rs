use std::fmt::Write;

pub struct Options<'a> {
    pub title: &'a str,
    pub crash: Option<i32>,
    pub hang: bool,
    pub stderr_flood: usize,
    pub session_probe: bool,
    pub config_probe: bool,
    pub canonical: bool,
}

pub fn parse<'a>(args: &[&'a str]) -> Result<Options<'a>, &'static str> {
    let title = *args.first().ok_or("missing window title")?;
    if title.is_empty() || title.starts_with("--") { return Err("invalid window title"); }
    let mut options = Options { title, crash: None, hang: false, stderr_flood: 0, session_probe: false, config_probe: false, canonical: false };
    let mut flood_seen = false;
    for arg in &args[1..] {
        if *arg == "--canonical" && !options.canonical {
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
