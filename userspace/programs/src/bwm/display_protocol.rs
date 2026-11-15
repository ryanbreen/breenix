//! The bounded, single-line display control protocol.
#[derive(Debug, PartialEq, Eq)]
pub enum Command { Mode(u32, u32), Scale(u32) }
#[derive(Clone, Copy)]
pub struct Rect { pub x: i32, pub y: i32, pub width: usize, pub height: usize }
pub struct Geometry { pub width: usize, pub height: usize, pub scale: usize, pub taskbar: Rect, pub appbar: Rect, pub work: Rect }

pub fn parse(line: &str) -> Result<Command, &'static str> {
    let line = line.strip_suffix('\n').ok_or("incomplete request")?;
    if let Some(value) = line.strip_prefix("mode ") {
        let (w, h) = value.split_once('x').ok_or("invalid mode")?;
        let number = |s: &str| if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) { s.parse::<u32>().ok().filter(|n| *n > 0) } else { None };
        return match (number(w), number(h)) { (Some(w), Some(h)) => Ok(Command::Mode(w, h)), _ => Err("invalid mode") };
    }
    match line { "scale 1" => Ok(Command::Scale(1)), "scale 2" => Ok(Command::Scale(2)), _ => Err("invalid request") }
}
pub fn reply(result: Result<Geometry, &str>) -> String {
    match result {
        Err(reason) => format!("refused {}\n", reason.replace(['\n', '\r'], " ")),
        Ok(g) => {
            let r = |r: Rect| format!("{},{},{},{}", r.x, r.y, r.width, r.height);
            format!("ok {}x{} scale {} taskbar {} appbar {} work {}\n", g.width, g.height, g.scale, r(g.taskbar), r(g.appbar), r(g.work))
        }
    }
}
