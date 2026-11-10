//! Build a GUI launch without shell parsing or joining catalog fields.
pub fn gui_argv(fields: &[String]) -> Vec<String> {
    fields.to_vec()
}

pub fn terminal_argv(fields: &[String]) -> Vec<String> {
    let mut argv = vec!["/usr/bin/bterm".into(), "-e".into()];
    argv.extend_from_slice(fields);
    argv
}

pub fn error_text(path: &str, missing: bool, error: &str) -> String {
    if missing { format!("Missing program: {path}") } else { error.to_owned() }
}
