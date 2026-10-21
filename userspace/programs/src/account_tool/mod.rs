//! Rust account tool argument handling; database operations call the accounts library.
use libbreenix::accounts::{self, UserAdd, UserMod};
use std::path::Path;

type Result<T> = std::result::Result<T, String>;
fn number(value: &str) -> Result<u32> {
    accounts::parse::id(value.as_bytes()).ok_or_else(|| "invalid numeric ID".into())
}
fn groups(value: &str) -> Result<Vec<String>> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.split(',').any(|v| {
        !accounts::parse::valid_name(v.as_bytes()) && accounts::parse::id(v.as_bytes()).is_none()
    }) {
        return Err("invalid group list".into());
    }
    Ok(value.split(',').map(str::to_owned).collect())
}
fn date(value: &str) -> Result<Option<u32>> {
    if value.is_empty() || value == "-1" {
        return Ok(None);
    }
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err("expiry must be YYYY-MM-DD or -1".into());
    }
    let year = number(parts[0])?;
    let month = number(parts[1])?;
    let day = number(parts[2])?;
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year < 1970 || !(1..=12).contains(&month) || day == 0 || day > lengths[(month - 1) as usize]
    {
        return Err("invalid expiry date".into());
    }
    let days: u32 = (1970..year).map(|y| if leap(y) { 366 } else { 365 }).sum();
    Ok(Some(
        days + lengths[..(month - 1) as usize].iter().sum::<u32>() + day - 1,
    ))
}
fn run(tool: &str, args: &[String]) -> Result<String> {
    accounts::require_root()?;
    let mut add = UserAdd::default();
    let mut modify = UserMod::default();
    let mut gid = None;
    let mut remove = false;
    let mut names = Vec::new();
    let mut i = 0;
    let mut options = true;
    while i < args.len() {
        let arg = args[i].as_str();
        if options && arg == "--" {
            options = false;
            i += 1;
            continue;
        }
        if options && arg.starts_with('-') {
            match (tool, arg) {
                ("useradd", "-m") => add.create_home = true,
                ("userdel", "-r") => remove = true,
                ("usermod", "-a") => modify.append = true,
                ("usermod", "-L") => modify.lock = true,
                ("usermod", "-U") => modify.unlock = true,
                ("useradd", "-u" | "-g" | "-G" | "-s" | "-d")
                | ("usermod", "-G" | "-s" | "-e")
                | ("groupadd", "-g") => {
                    i += 1;
                    let value = args.get(i).ok_or("option requires an argument")?;
                    match (tool, arg) {
                        ("useradd", "-u") => add.uid = Some(number(value)?),
                        ("useradd", "-g") => add.group = Some(value.clone()),
                        ("useradd", "-G") => add.groups = groups(value)?,
                        ("useradd", "-s") => add.shell = Some(value.clone()),
                        ("useradd", "-d") => add.home = Some(value.clone()),
                        ("usermod", "-G") => modify.groups = Some(groups(value)?),
                        ("usermod", "-s") => modify.shell = Some(value.clone()),
                        ("usermod", "-e") => modify.expire = Some(date(value)?),
                        ("groupadd", "-g") => gid = Some(number(value)?),
                        _ => unreachable!(),
                    }
                }
                _ => return Err(format!("unknown option {arg}")),
            }
        } else {
            names.push(arg);
        }
        i += 1;
    }
    if names.len() != 1 || !accounts::parse::valid_name(names[0].as_bytes()) {
        return Err("supply exactly one valid account or group name".into());
    }
    let root = Path::new("/");
    let name = names[0];
    match tool {
        "useradd" => {
            accounts::useradd(root, name, &add)?;
        }
        "userdel" => accounts::userdel(root, name, remove)?,
        "usermod" => accounts::usermod(root, name, &modify)?,
        "groupadd" => accounts::groupadd(root, name, gid)?,
        "groupdel" => accounts::groupdel(root, name)?,
        _ => return Err("unknown tool".into()),
    }
    Ok(name.into())
}
pub fn main(tool: &str) {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = run(tool, &args);
    let target = result
        .as_deref()
        .unwrap_or_else(|_| args.last().map(String::as_str).unwrap_or("-"));
    let logged = accounts::audit(Path::new("/"), tool, target, result.is_ok());
    if let Err(e) = &result {
        eprintln!("{tool}: {e}");
    }
    if let Err(e) = &logged {
        eprintln!("{tool}: audit: {e}");
    }
    std::process::exit(if result.is_ok() && logged.is_ok() {
        0
    } else {
        1
    });
}
