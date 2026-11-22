//! Resolve user-defined, single-step POSIX suffix inference to a recipe and its source.

use crate::reader::{Entry, Inference, Makefile, Rule};
use std::{
    collections::{HashMap, HashSet},
    fs,
};

pub(crate) struct Suffixes(Vec<String>);

impl Suffixes {
    pub(crate) fn resolve(makefile: &mut Makefile, goals: &[String]) -> Self {
        let mut suffixes = Self(
            [".o", ".c", ".y", ".l", ".a", ".sh"]
                .map(str::to_owned)
                .to_vec(),
        );
        makefile.entries.retain(|entry| {
            let Entry::Rule(rule) = entry else {
                return true;
            };
            match rule.targets.as_slice() {
                [name] if name == ".SUFFIXES" => {
                    if rule.prerequisites.is_empty() {
                        suffixes.0.clear();
                    } else {
                        for suffix in &rule.prerequisites {
                            if !suffixes.0.contains(suffix) {
                                suffixes.0.push(suffix.clone());
                            }
                        }
                    }
                    false
                }
                [name] if name == ".POSIX" => false,
                _ => true,
            }
        });

        let mut inference = HashMap::new();
        let mut explicit = Vec::new();
        let mut fallback = None;
        for entry in std::mem::take(&mut makefile.entries) {
            match entry {
                Entry::Rule(rule) if rule.targets == [".DEFAULT"] => fallback = Some(rule),
                Entry::Rule(rule) if suffixes.is_inference(&rule) => {
                    inference.insert(rule.targets[0].clone(), rule);
                }
                other => explicit.push(other),
            }
        }

        let mut targets = HashMap::new();
        let mut prerequisites = HashSet::new();
        for entry in &explicit {
            if let Entry::Rule(rule) = entry {
                prerequisites.extend(rule.prerequisites.iter().map(String::as_str));
                for target in &rule.targets {
                    let has_recipe = targets.entry(target.as_str()).or_insert(false);
                    *has_recipe |= !rule.recipes.is_empty();
                }
            }
        }
        prerequisites.extend(targets.keys().copied());
        prerequisites.extend(goals.iter().map(String::as_str));
        // Synthesized rules are appended so inference cannot change the default goal.
        let mut candidates: Vec<_> = prerequisites.into_iter().collect();
        candidates.sort_unstable();
        let derived: Vec<_> = candidates
            .into_iter()
            .filter_map(|target| {
                let has_rule = targets.get(target).copied();
                if has_rule == Some(true) {
                    return None;
                }
                if let Some(rule) = suffixes.infer(target, has_rule.is_some(), &inference) {
                    return Some(Entry::Rule(rule));
                }
                let fallback = fallback.as_ref()?;
                if has_rule.is_none() && !exists(target) {
                    return Some(Entry::Rule(Rule {
                        location: fallback.location.clone(),
                        targets: vec![target.into()],
                        prerequisites: Vec::new(),
                        recipes: fallback.recipes.clone(),
                        inference: Some(Inference {
                            source: target.into(),
                            stem: suffixes.stem(target).into(),
                        }),
                    }));
                }
                None
            })
            .collect();
        explicit.extend(derived);
        makefile.entries = explicit;
        suffixes
    }

    fn is_inference(&self, rule: &Rule) -> bool {
        let [name] = rule.targets.as_slice() else {
            return false;
        };
        let Some(suffix) = name.strip_prefix('.').filter(|name| !name.contains('/')) else {
            return false;
        };
        match suffix.find('.') {
            Some(index) => self.known(&name[..index + 1]) && self.known(&suffix[index..]),
            None => self.known(name),
        }
    }

    fn known(&self, suffix: &str) -> bool {
        self.0.iter().any(|known| known == suffix)
    }

    fn suffix<'a>(&self, target: &'a str) -> Option<(&'a str, &'a str)> {
        let (stem, extension) = target.rsplit_once('.')?;
        let suffix = &target[stem.len()..];
        (!extension.contains('/') && self.known(suffix)).then_some((stem, suffix))
    }

    pub(crate) fn stem<'a>(&self, target: &'a str) -> &'a str {
        self.suffix(target).map_or(target, |(stem, _)| stem)
    }

    fn infer(&self, target: &str, has_rule: bool, rules: &HashMap<String, Rule>) -> Option<Rule> {
        let (name, member) = archive_parts(target);
        let (stem, suffix) = if let Some(member) = member {
            (member.strip_suffix(".o")?, ".a")
        } else if let Some(parts) = self.suffix(name) {
            parts
        } else if !has_rule && !name.rsplit('/').next()?.contains('.') {
            (name, "")
        } else {
            return None;
        };
        if !suffix.is_empty() && !self.known(suffix) {
            return None;
        }
        for source_suffix in &self.0 {
            let Some(rule) = rules.get(&format!("{source_suffix}{suffix}")) else {
                continue;
            };
            let source = format!("{stem}{source_suffix}");
            if exists(&source) {
                return Some(Rule {
                    location: rule.location.clone(),
                    targets: vec![target.into()],
                    prerequisites: vec![source.clone()],
                    recipes: rule.recipes.clone(),
                    inference: Some(Inference {
                        source,
                        stem: stem.into(),
                    }),
                });
            }
        }
        None
    }
}

fn exists(path: &str) -> bool {
    fs::metadata(path).is_ok()
}

pub(crate) fn archive_parts(target: &str) -> (&str, Option<&str>) {
    if let Some((archive, member)) = target
        .strip_suffix(')')
        .and_then(|name| name.split_once('('))
    {
        return (archive, Some(member));
    }
    (target, None)
}
