//! Build a GUI launch without shell parsing or joining catalog fields.
pub fn gui_argv(fields: &[String]) -> Vec<String> {
    fields.to_vec()
}
