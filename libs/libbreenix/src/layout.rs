//! The image builder, installer and users suite share this layout table.
pub const TABLE: &str = include_str!("layout.tsv");
pub const SHARED: [&str; 6] = ["etc", "var", "opt", "srv", "tmp", "run"];

/// Check on-disk set-ID metadata against the design's explicit placement rows.
/// Virtual mounts have no image inodes; symlinks are inspected without following them.
pub fn check_setid(root: &std::path::Path, virtual_mounts: bool) -> Result<(), String> {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    fn walk(root: &Path, path: &Path, virtual_mounts: bool) -> Result<(), String> {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        if meta.mode() & 0o6000 != 0 {
            let allowed = TABLE.lines().any(|line| {
                let row: Vec<_> = line.split_whitespace().collect();
                row.len() == 5 && row[0] == "bin" && !row[1].contains('*')
                    && relative == Path::new(row[2]).join(row[1])
                    && meta.is_file() && meta.uid() == 0 && meta.gid() == 0
                    && u32::from_str_radix(row[3], 8) == Ok(meta.mode() & 0o7777)
            });
            if !allowed { return Err(format!("{}: unlisted set-ID inode (mode {:o})", path.display(), meta.mode() & 0o7777)); }
        }
        if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|e| format!("{}: {}", path.display(), e))? {
                let child = entry.map_err(|e| e.to_string())?.path();
                if virtual_mounts && path == root && matches!(child.file_name().and_then(|s| s.to_str()), Some("dev" | "proc" | "sys")) { continue; }
                walk(root, &child, virtual_mounts)?;
            }
        }
        Ok(())
    }
    walk(root, root, virtual_mounts)
}

pub fn record(id: &str) -> String {
    format!("BREENIX-USERLAND 2\n{}\nshared etc var opt srv tmp run\n", id)
}
