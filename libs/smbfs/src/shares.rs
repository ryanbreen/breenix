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

/// Select shares whose directories are not already mounted.
pub fn plan_mount_all<'a>(shares: &'a [Share], mounts: &str) -> Vec<&'a Share> {
    let mounted: Vec<_> = mounts.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        fields.next()?;
        Some(decode_mount_field(fields.next()?))
    }).collect();
    shares.iter().filter(|share| !mounted.contains(&share.directory)).collect()
}

/// Render the source, directory, type and options from each proc mount entry.
pub fn render_proc_mounts(text: &str) -> String {
    let mut output = String::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(source), Some(directory), Some(kind), Some(options)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let source = decode_mount_field(source);
        let directory = decode_mount_field(directory);
        let kind = decode_mount_field(kind);
        let options = decode_mount_field(options);
        let kind = if kind == "ufs" && source.starts_with("//") {
            "smb"
        } else {
            &kind
        };
        output.push_str(&format!(
            "{source} on {directory} type {kind} ({options})\n"
        ));
    }
    output
}

fn decode_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && (b'0'..=b'3').contains(&bytes[i + 1])
            && (b'0'..=b'7').contains(&bytes[i + 2])
            && (b'0'..=b'7').contains(&bytes[i + 3])
        {
            decoded
                .push((bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + bytes[i + 3] - b'0');
            i += 4;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
