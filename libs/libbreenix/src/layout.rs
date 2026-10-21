//! The image builder, installer and users suite share this layout table.
pub const TABLE: &str = include_str!("layout.tsv");
pub const SHARED: [&str; 6] = ["etc", "var", "opt", "srv", "tmp", "run"];

pub fn record(id: &str) -> String {
    format!("BREENIX-USERLAND 2\n{}\nshared etc var opt srv tmp run\n", id)
}
