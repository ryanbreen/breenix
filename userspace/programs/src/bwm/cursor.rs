//! Cursor policy read only from a private UI session configuration root.
use std::{fs, path::Path};

/// No session option means the ordinary desktop's visible cursor. An ordinary
/// desktop does not consult this file, even if one exists under /etc.
pub fn load(root: Option<&Path>) -> Result<bool, String> {
    let Some(root) = root else { return Ok(true); };
    let path = root.join("etc/uitest-cursor.conf");
    match fs::read_to_string(&path) {
        Ok(text) => match text.as_str() {
            "visible=true\n" => Ok(true),
            "visible=false\n" => Ok(false),
            _ => Err(format!("{}: expected visible=true or visible=false with newline",path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("{}: {error}",path.display())),
    }
}

/// Use the same selection for drawing, visibility analysis and pixel validation.
pub fn choose<T>(visible: bool, pointer: T) -> Option<T> { visible.then_some(pointer) }
