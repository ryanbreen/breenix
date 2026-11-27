//! Per-program suite manifests, fixture trees and byte-exact golden comparisons.

mod fixture;
mod golden;
pub mod json;

pub use fixture::Fixture;
pub use golden::{first_difference, Goldens, MAX_OUTPUT};

use json::Json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub struct Suite {
    pub program: String,
    pub binary: String,
    pub cases: Vec<Case>,
}

pub struct Case {
    pub id: String,
    pub argv: Vec<String>,
    pub fixture: Option<String>,
    pub env: BTreeMap<String, String>,
    pub timeout: Duration,
    pub stdout: Stdout,
}

#[derive(Clone, Copy)]
pub enum Stdout {
    Capture,
    Prefix(usize),
    Closed,
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

impl Suite {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut fields = Fields::new(Json::parse(text)?)?;
        if fields.integer("format")? != Some(1) {
            return Err("suite format must be 1".into());
        }
        let program = fields.name("program")?;
        let binary = fields.name("binary")?;
        fields.string("provenance")?;
        let mut seen = BTreeSet::new();
        let cases = fields
            .array("cases")?
            .into_iter()
            .map(Case::parse)
            .map(|case| {
                let case = case?;
                if !seen.insert(case.id.clone()) {
                    return Err(format!("duplicate case {}", case.id));
                }
                Ok(case)
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields.finish()?;
        if cases.is_empty() {
            return Err("suite has no cases".into());
        }
        Ok(Self {
            program,
            binary,
            cases,
        })
    }
}

impl Case {
    fn parse(value: Json) -> Result<Self, String> {
        let mut fields = Fields::new(value)?;
        let id = fields.name("id")?;
        let source = fields.required_string("source")?;
        if source != "gnu" && source != "manual" {
            return Err(format!("{id}: source must be gnu or manual"));
        }
        let argv = fields
            .array("argv")?
            .into_iter()
            .map(|value| as_string("argv entry", value))
            .collect::<Result<_, _>>()?;
        let fixture = fields.string("fixture")?;
        if let Some(name) = &fixture {
            if !identifier(name) {
                return Err(format!("{id}: invalid fixture name"));
            }
        }
        let timeout_ms = fields.integer("timeout_ms")?.unwrap_or(5000);
        if !(1..=60_000).contains(&timeout_ms) {
            return Err(format!("{id}: timeout_ms must be in 1..=60000"));
        }
        let stdout = match (
            fields.integer("stdout_prefix_bytes")?,
            fields.boolean("stdout_closed")?,
        ) {
            (Some(n), false) if (1..=MAX_OUTPUT as u64).contains(&n) => Stdout::Prefix(n as usize),
            (None, true) => Stdout::Closed,
            (None, false) => Stdout::Capture,
            _ => return Err(format!("{id}: invalid stdout settings")),
        };
        let mut env = BTreeMap::new();
        if let Some(value) = fields.0.remove("env") {
            for (name, value) in Fields::new(value)?.0 {
                if name.is_empty() || name.contains(['=', '\0']) {
                    return Err(format!("{id}: invalid environment variable {name:?}"));
                }
                env.insert(name, as_string("environment value", value)?);
            }
        }
        fields.finish()?;
        Ok(Self {
            id,
            argv,
            fixture,
            env,
            timeout: Duration::from_millis(timeout_ms),
            stdout,
        })
    }
}

fn as_string(key: &str, value: Json) -> Result<String, String> {
    match value {
        Json::String(value) if !value.contains('\0') => Ok(value),
        _ => Err(format!("{key} must be a string without NUL")),
    }
}

struct Fields(BTreeMap<String, Json>);

impl Fields {
    fn new(value: Json) -> Result<Self, String> {
        match value {
            Json::Object(fields) => Ok(Self(fields)),
            _ => Err("expected a JSON object".into()),
        }
    }

    fn name(&mut self, key: &str) -> Result<String, String> {
        let value = self.required_string(key)?;
        if !identifier(&value) {
            return Err(format!("{key} must be kebab-case"));
        }
        Ok(value)
    }

    fn required_string(&mut self, key: &str) -> Result<String, String> {
        self.string(key)?.ok_or_else(|| format!("missing {key}"))
    }

    fn string(&mut self, key: &str) -> Result<Option<String>, String> {
        match self.0.remove(key) {
            None | Some(Json::Null) => Ok(None),
            Some(value) => as_string(key, value).map(Some),
        }
    }

    fn array(&mut self, key: &str) -> Result<Vec<Json>, String> {
        match self.0.remove(key) {
            Some(Json::Array(items)) => Ok(items),
            _ => Err(format!("{key} must be an array")),
        }
    }

    fn integer(&mut self, key: &str) -> Result<Option<u64>, String> {
        match self.0.remove(key) {
            None => Ok(None),
            Some(Json::Number(value)) => value
                .parse()
                .map(Some)
                .map_err(|_| format!("{key} must be a nonnegative integer")),
            _ => Err(format!("{key} must be a nonnegative integer")),
        }
    }

    fn boolean(&mut self, key: &str) -> Result<bool, String> {
        match self.0.remove(key) {
            None => Ok(false),
            Some(Json::Bool(flag)) => Ok(flag),
            _ => Err(format!("{key} must be a boolean")),
        }
    }

    fn finish(self) -> Result<(), String> {
        if let Some(key) = self.0.keys().next() {
            return Err(format!("unsupported field {key:?}"));
        }
        Ok(())
    }
}
