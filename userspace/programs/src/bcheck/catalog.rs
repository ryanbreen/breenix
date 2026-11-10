//! Parsing for the self-check catalog; each invalid line remains a result row.

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub category: String,
    pub deadline_ms: u64,
    pub argv: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub text: String,
    pub reason: &'static str,
}

pub fn parse(text: &str) -> Vec<Result<Entry, ParseError>> {
    text.lines().enumerate().filter_map(|(index, line)| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let fields: Vec<_> = trimmed.split_whitespace().collect();
        let parsed = (|| {
            if fields.len() < 4 { return Err("too few fields"); }
            let deadline_ms = fields[2].parse::<u64>().map_err(|_| "invalid deadline")?;
            if deadline_ms == 0 { return Err("zero deadline"); }
            if !fields[3].starts_with('/') { return Err("relative path"); }
            Ok(Entry {
                name: fields[0].to_owned(),
                category: fields[1].to_owned(),
                deadline_ms,
                argv: fields[3..].iter().map(|field| (*field).to_owned()).collect(),
            })
        })();
        Some(parsed.map_err(|reason| ParseError { line: index + 1, text: line.to_owned(), reason }))
    }).collect()
}
