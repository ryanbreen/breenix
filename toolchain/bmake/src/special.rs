//! POSIX.1-2024 make special targets that control recipe execution.

use crate::{
    reader::{Entry, Makefile},
    Error,
};
use std::collections::HashSet;

#[derive(Default)]
pub(crate) struct Specials {
    pub phony: HashSet<String>,
    pub ignore: Targets,
    pub silent: Targets,
    pub precious: Targets,
    pub not_parallel: bool,
}

#[derive(Default)]
pub(crate) struct Targets {
    all: bool,
    names: HashSet<String>,
}

impl Targets {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.all || self.names.contains(name)
    }

    fn add(&mut self, names: &[String]) {
        self.all |= names.is_empty();
        self.names.extend(names.iter().cloned());
    }
}

impl Specials {
    pub(crate) fn take(makefile: &mut Makefile) -> Result<Self, Error> {
        let mut specials = Self::default();
        let mut entries = Vec::new();
        for entry in std::mem::take(&mut makefile.entries) {
            let Entry::Rule(rule) = &entry else {
                entries.push(entry);
                continue;
            };
            let Some(name) = rule.targets.iter().find(|name| {
                matches!(
                    name.as_str(),
                    ".POSIX"
                        | ".PHONY"
                        | ".IGNORE"
                        | ".SILENT"
                        | ".PRECIOUS"
                        | ".NOTPARALLEL"
                        | ".WAIT"
                )
            }) else {
                entries.push(entry);
                continue;
            };
            if rule.targets.len() != 1 || !rule.recipes.is_empty() {
                return Err(Error::new(
                    &rule.location,
                    format!("{name} must stand alone and have no recipe"),
                ));
            }
            match name.as_str() {
                ".PHONY" => specials.phony.extend(rule.prerequisites.iter().cloned()),
                ".IGNORE" => specials.ignore.add(&rule.prerequisites),
                ".SILENT" => specials.silent.add(&rule.prerequisites),
                ".PRECIOUS" => specials.precious.add(&rule.prerequisites),
                // The supported rules are used with or without a .POSIX declaration.
                ".POSIX" | ".NOTPARALLEL" | ".WAIT" if !rule.prerequisites.is_empty() => {
                    return Err(Error::new(
                        &rule.location,
                        format!("{name} must have no prerequisites"),
                    ));
                }
                ".NOTPARALLEL" => specials.not_parallel = true,
                _ => (),
            }
        }
        makefile.entries = entries;
        Ok(specials)
    }
}
