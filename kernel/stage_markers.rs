//! The serial markers listed by the main and metal boot paths.
//!
//! A milestone names its stages; their markers are in the boot-stage catalog
//! (`xtask/src/boot_stages.rs`), read here from its committed exports in
//! `tools/breenix-runs/Sources/BreenixRuns/Resources/`, which a host test keeps
//! equal to the live catalog. A stage listed with its own marker carries it
//! inline. A list keyed by an arch resolves against that arch's catalog; a
//! main-path mode resolves against both catalogs; a modes-scored path uses
//! its gateArch catalog. Host events do not have guest serial markers.
//! A `|` separates alternative markers.
//!
//! `kernel/build.rs` compiles these markers into the kernel (`stage_lines`),
//! so the kernel's output paths can tell a line a stage depends on from any
//! other. The host test `tests/stage_line_pressure.rs` loads them the same way.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const ARCHES: [&str; 2] = ["aarch64", "x86_64"];
const BOOT_PATHS: [&str; 2] = ["docs/boot-path.json", "docs/metal/boot-path.json"];

/// The longest marker the kernel's scanner matches (`stage_lines::MAX_MARKER`).
pub const MAX_MARKER: usize = 128;

/// The files the markers are read from, relative to the repository root.
pub fn sources(repo: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = BOOT_PATHS.iter().map(|path| repo.join(path)).collect();
    files.extend(ARCHES.iter().map(|arch| catalog_path(repo, arch)));
    files
}

fn catalog_path(repo: &Path, arch: &str) -> PathBuf {
    repo.join(format!(
        "tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-{arch}.json"
    ))
}

fn read_json(path: &Path) -> Result<Json, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    let value = parser
        .value()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(format!(
            "{}: trailing text at byte {}",
            path.display(),
            parser.at
        ));
    }
    Ok(value)
}

/// A JSON value. The kernel's build script has no JSON library: an artifact
/// dependency's build-dependencies are built for the kernel's target.
enum Json {
    Null,
    Bool,
    Number,
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

static NULL: Json = Json::Null;

impl Json {
    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map_or(&NULL, |(_, value)| value),
            _ => &NULL,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(fields) => Some(fields),
            _ => None,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    fn next(&mut self) -> Result<u8, String> {
        let byte = *self.bytes.get(self.at).ok_or("unexpected end of JSON")?;
        self.at += 1;
        Ok(byte)
    }

    fn expect(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(format!("expected {word} at byte {}", self.at))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.bytes.get(self.at).copied() {
            Some(b'{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(fields));
                }
                loop {
                    self.space();
                    self.expect("\"")?;
                    let name = self.string()?;
                    self.space();
                    self.expect(":")?;
                    fields.push((name, self.value()?));
                    self.space();
                    match self.next()? {
                        b',' => {}
                        b'}' => return Ok(Json::Object(fields)),
                        _ => return Err(format!("expected , or }} at byte {}", self.at - 1)),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.space();
                    match self.next()? {
                        b',' => {}
                        b']' => return Ok(Json::Array(items)),
                        _ => return Err(format!("expected , or ] at byte {}", self.at - 1)),
                    }
                }
            }
            Some(b'"') => {
                self.at += 1;
                Ok(Json::Str(self.string()?))
            }
            Some(b't') => self.expect("true").map(|()| Json::Bool),
            Some(b'f') => self.expect("false").map(|()| Json::Bool),
            Some(b'n') => self.expect("null").map(|()| Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|b| b.is_ascii_digit() || b"+-.eE".contains(b))
                {
                    self.at += 1;
                }
                Ok(Json::Number)
            }
            _ => Err(format!("unexpected byte at {}", self.at)),
        }
    }

    /// The rest of a string whose opening quote is consumed.
    fn string(&mut self) -> Result<String, String> {
        let mut text = Vec::new();
        loop {
            match self.next()? {
                b'"' => return String::from_utf8(text).map_err(|e| e.to_string()),
                b'\\' => {
                    let escaped = match self.next()? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut unit = self.hex4()?;
                            if (0xd800..0xdc00).contains(&unit) {
                                self.expect("\\u")?;
                                let low = self.hex4()?;
                                unit = 0x10000
                                    + ((unit - 0xd800) << 10)
                                    + (low.wrapping_sub(0xdc00) & 0x3ff);
                            }
                            char::from_u32(unit).ok_or("invalid \\u escape")?
                        }
                        other => return Err(format!("invalid escape \\{}", other as char)),
                    };
                    let mut utf8 = [0u8; 4];
                    text.extend_from_slice(escaped.encode_utf8(&mut utf8).as_bytes());
                }
                byte => text.push(byte),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or("short \\u escape")?;
        let digits = std::str::from_utf8(digits).map_err(|e| e.to_string())?;
        self.at += 4;
        u32::from_str_radix(digits, 16).map_err(|e| e.to_string())
    }
}

fn split(marker: &str, into: &mut BTreeSet<String>) {
    into.extend(
        marker
            .split('|')
            .filter(|part| !part.is_empty())
            .map(str::to_owned),
    );
}

/// The serial markers from both boot paths, deduplicated and sorted
/// by last byte as `stage_lines` looks them up.
pub fn boot_path_markers(repo: &Path) -> Result<Vec<String>, String> {
    let mut catalogs: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    for arch in ARCHES {
        let catalog = read_json(&catalog_path(repo, arch))?;
        let stages = catalog
            .get("stages")
            .as_array()
            .ok_or_else(|| format!("{arch} catalog has no stages array"))?;
        let names = stages
            .iter()
            .filter_map(|stage| {
                Some((
                    stage.get("name").as_str()?.to_owned(),
                    stage.get("marker").as_str()?.to_owned(),
                ))
            })
            .collect();
        catalogs.insert(arch, names);
    }

    let mut markers = BTreeSet::new();
    for file in BOOT_PATHS {
        let path = repo.join(file);
        let boot_path = read_json(&path)?;
        let milestones = boot_path
            .get("milestones")
            .as_array()
            .ok_or_else(|| format!("{}: no milestones array", path.display()))?;

        for milestone in milestones {
            let id = milestone.get("id").as_str().unwrap_or("?");
            let lists = milestone
                .get("stages")
                .as_object()
                .ok_or_else(|| format!("milestone {id}: stages is not an object"))?;
            for (key, list) in lists {
                let arches: Vec<&str> = match ARCHES.iter().find(|arch| **arch == key.as_str()) {
                    Some(arch) => vec![*arch],
                    None if boot_path.get("scoring").as_str() == Some("modes") => {
                        let arch = boot_path
                            .get("gateArch")
                            .as_str()
                            .ok_or_else(|| format!("{file}: missing gateArch"))?;
                        if !catalogs.contains_key(arch) {
                            return Err(format!("{file}: unknown gateArch {arch}"));
                        }
                        vec![arch]
                    }
                    None => ARCHES.to_vec(),
                };
                let list = list
                    .as_array()
                    .ok_or_else(|| format!("milestone {id}: stages.{key} is not a list"))?;
                for stage in list {
                    if let Some(name) = stage.as_str() {
                        let mut found = false;
                        for arch in &arches {
                            if let Some(marker) = catalogs[arch].get(name) {
                                split(marker, &mut markers);
                                found = true;
                            }
                        }
                        if !found {
                            return Err(format!(
                                "milestone {id}: stage {name:?} ({key}) is not in the {} catalog",
                                arches.join(" or ")
                            ));
                        }
                    } else if let Some(marker) = stage.get("marker").as_str() {
                        split(marker, &mut markers);
                    } else if stage.get("host").as_str().is_some() {
                        // Host events have no guest serial marker to protect.
                    } else {
                        return Err(format!(
                            "milestone {id}: stages.{key} has an entry with no marker"
                        ));
                    }
                }
            }
        }
    }
    if let Some(long) = markers.iter().find(|marker| marker.len() > MAX_MARKER) {
        return Err(format!("marker {long:?} is longer than {MAX_MARKER} bytes"));
    }
    let mut markers: Vec<String> = markers.into_iter().collect();
    markers.sort_by(|a, b| (a.as_bytes().last(), a).cmp(&(b.as_bytes().last(), b)));
    Ok(markers)
}

/// ARM64-applicable stages, in the boot-path file's order (not marker sort order).
pub fn arm64_ribbon(repo:&Path)->Result<Vec<String>,String> {
    let path=read_json(&repo.join("docs/boot-path.json"))?;
    let catalog=read_json(&catalog_path(repo,"aarch64"))?;
    let names:BTreeMap<_,_>=catalog.get("stages").as_array().ok_or("no stages")?.iter()
        .filter_map(|s|Some((s.get("name").as_str()?,s.get("marker").as_str()?))).collect();
    let mut out=Vec::new();
    for m in path.get("milestones").as_array().ok_or("no milestones")? {
        if let Some(stages)=m.get("stages").get("aarch64").as_array() {
            for s in stages {
                let marker=if let Some(name)=s.as_str(){*names.get(name).ok_or_else(||format!("unknown stage {name}"))?}
                    else{s.get("marker").as_str().ok_or("missing marker")?};
                out.push(marker.to_owned());
            }
        }
    }
    Ok(out)
}
