//! Parser for the boot-time SMB shares table.

/// A share and the mount arguments to pass to the SMB mount tool.
#[derive(Debug, PartialEq, Eq)]
pub struct Share {
    pub source: String,
    pub directory: String,
    pub options: String,
}

/// A rejected table line and the reason it was rejected.
#[derive(Debug, PartialEq, Eq)]
pub struct ShareError {
    pub line: usize,
    pub message: &'static str,
}

/// Parse each non-comment line independently, preserving valid entries and errors.
pub fn parse_shares(text: &str) -> (Vec<Share>, Vec<ShareError>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let result = match fields.as_slice() {
            [source, directory, kind, options] => {
                if *kind != "smb" {
                    Err("invalid filesystem type")
                } else if !directory.starts_with('/') {
                    Err("mount directory must be absolute")
                } else {
                    crate::parse_mount_options(source, options).map(|_| Share {
                        source: (*source).to_owned(),
                        directory: (*directory).to_owned(),
                        options: (*options).to_owned(),
                    })
                }
            }
            _ => Err("expected four fields"),
        };
        match result {
            Ok(entry) => entries.push(entry),
            Err(message) => errors.push(ShareError {
                line: index + 1,
                message,
            }),
        }
    }
    (entries, errors)
}
