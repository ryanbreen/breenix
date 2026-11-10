//! Launcher catalog parsing and search, independent of the GUI.

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Gui,
    Terminal,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub mode: Mode,
    pub argv: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub reason: String,
}

pub fn parse(text: &str) -> (Vec<Entry>, Vec<ParseError>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        let error = if fields.len() < 4 {
            Some("too few fields")
        } else if fields[0].trim().is_empty() {
            Some("empty name")
        } else if fields[2] != "gui" && fields[2] != "terminal" {
            Some("unknown mode")
        } else if !fields[3].starts_with('/') {
            Some("relative argv[0]")
        } else {
            None
        };
        if let Some(reason) = error {
            errors.push(ParseError { line: index + 1, reason: reason.to_owned() });
            continue;
        }
        entries.push(Entry {
            name: fields[0].to_owned(),
            description: fields[1].to_owned(),
            mode: if fields[2] == "gui" { Mode::Gui } else { Mode::Terminal },
            argv: fields[3..].iter().map(|field| (*field).to_owned()).collect(),
        });
    }
    (entries, errors)
}

pub fn filter<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let query = query.to_lowercase();
    entries.iter().filter(|entry| {
        entry.name.to_lowercase().contains(&query)
            || entry.description.to_lowercase().contains(&query)
    }).collect()
}
