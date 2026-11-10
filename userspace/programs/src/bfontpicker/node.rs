use serde_json::{json, Value};

pub struct Row<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub section: &'a str,
    pub usable: bool,
    pub active: bool,
}

pub fn fonts(rows: &[Row<'_>], selected: Option<usize>) -> Value {
    json!({"rows": rows.iter().map(|r| json!({"name":r.name,"path":r.path,"section":r.section,"usable":r.usable,"active":r.active})).collect::<Vec<_>>(), "selected":selected})
}

pub fn status(mono: &str, mono_size: f32, display: &str, display_size: f32, save: Option<&str>, error: Option<&str>) -> Value {
    json!({"mono":{"path":mono,"size":mono_size},"display":{"path":display,"size":display_size},"save":save,"error":error})
}
