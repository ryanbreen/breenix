//! Single-step suffix inference from POSIX.1-2024 make, "Inference Rules"
//! and "Default Rules". The assembler rule is a Breenix extension.

use crate::{
    graph::archive_member,
    reader::{Entry, Inference, Makefile, Rule, SourceLine},
    special::Specials,
    Error, Location,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const DEFAULT_SUFFIXES: &[&str] = &[".o", ".c", ".y", ".l", ".a", ".sh", ".s"];

const DEFAULT_RULES: &[(&str, &[&str])] = &[
    (".c", &["$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<"]),
    (".sh", &["cp $< $@", "chmod a+x $@"]),
    (".c.o", &["$(CC) $(CFLAGS) -c $<"]),
    (
        ".y.o",
        &[
            "$(YACC) $(YFLAGS) $<",
            "$(CC) $(CFLAGS) -c y.tab.c",
            "rm -f y.tab.c",
            "mv y.tab.o $@",
        ],
    ),
    (
        ".l.o",
        &[
            "$(LEX) $(LFLAGS) $<",
            "$(CC) $(CFLAGS) -c lex.yy.c",
            "rm -f lex.yy.c",
            "mv lex.yy.o $@",
        ],
    ),
    (".y.c", &["$(YACC) $(YFLAGS) $<", "mv y.tab.c $@"]),
    (".l.c", &["$(LEX) $(LFLAGS) $<", "mv lex.yy.c $@"]),
    (
        ".c.a",
        &[
            "$(CC) -c $(CFLAGS) $<",
            "$(AR) $(ARFLAGS) $@ $*.o",
            "rm -f $*.o",
        ],
    ),
    (".s.o", &["$(AS) $(ASFLAGS) -o $@ $<"]),
];

pub(crate) struct Suffixes(Vec<String>);

impl Suffixes {
    pub(crate) fn resolve(
        makefile: &mut Makefile,
        goals: &[String],
        specials: &Specials,
    ) -> Result<Self, Error> {
        let mut suffixes = Self(
            DEFAULT_SUFFIXES
                .iter()
                .map(|suffix| (*suffix).into())
                .collect(),
        );
        // A later clear disables rule selection without turning earlier
        // inference definitions into ordinary targets.
        let mut declared = Self(suffixes.0.clone());
        let mut explicit = Vec::new();
        for entry in std::mem::take(&mut makefile.entries) {
            if let Entry::Rule(rule) = &entry {
                if rule.targets.iter().any(|name| name == ".SUFFIXES") {
                    if rule.targets.len() != 1 || !rule.recipes.is_empty() {
                        return Err(Error::new(
                            &rule.location,
                            ".SUFFIXES must stand alone and have no recipe",
                        ));
                    }
                    if rule.prerequisites.is_empty() {
                        suffixes.0.clear();
                    } else {
                        for suffix in &rule.prerequisites {
                            if !suffixes.known(suffix) {
                                suffixes.0.push(suffix.clone());
                            }
                            if !declared.known(suffix) {
                                declared.0.push(suffix.clone());
                            }
                        }
                    }
                    continue;
                }
            }
            explicit.push(entry);
        }

        let mut inference = builtin_rules();
        let mut fallback = None;
        let mut entries = Vec::new();
        for entry in explicit {
            match entry {
                Entry::Rule(rule) if rule.targets.iter().any(|name| name == ".DEFAULT") => {
                    if rule.targets.len() != 1
                        || !rule.prerequisites.is_empty()
                        || rule.recipes.is_empty()
                    {
                        return Err(Error::new(
                            &rule.location,
                            ".DEFAULT must stand alone and have a recipe without prerequisites",
                        ));
                    }
                    fallback = Some(rule);
                }
                Entry::Rule(rule) if declared.is_inference(&rule) => {
                    if !rule.prerequisites.is_empty() {
                        return Err(Error::new(
                            &rule.location,
                            "suffix inference rules must have no explicit prerequisites",
                        ));
                    }
                    inference.insert(rule.targets[0].clone(), rule);
                }
                other => entries.push(other),
            }
        }

        let mut targets = HashMap::new();
        let mut candidates = HashSet::new();
        for entry in &entries {
            if let Entry::Rule(rule) = entry {
                candidates.extend(
                    rule.prerequisites
                        .iter()
                        .chain(&rule.order_only)
                        .map(String::as_str),
                );
                for target in &rule.targets {
                    let has_recipe = targets.entry(target.as_str()).or_insert(false);
                    *has_recipe |= !rule.recipes.is_empty();
                }
            }
        }
        candidates.extend(targets.keys().copied());
        candidates.extend(goals.iter().map(String::as_str));
        candidates.extend(
            makefile
                .includes
                .iter()
                .map(|include| include.name.as_str()),
        );
        // Appending derived rules preserves prerequisite and default-goal order.
        let mut candidates: Vec<_> = candidates.into_iter().collect();
        candidates.sort_unstable();
        let mut derived = Vec::new();
        for target in candidates {
            if target == ".WAIT" {
                continue;
            }
            let has_recipe = targets.get(target).copied();
            if has_recipe == Some(true) || specials.phony.contains(target) {
                continue;
            }
            if let Some(rule) = suffixes.infer(target, has_recipe.is_some(), &inference) {
                derived.push(Entry::Rule(rule));
            } else if let Some(rule) = &fallback {
                if has_recipe.is_none() && !Path::new(target).exists() {
                    derived.push(Entry::Rule(derived_rule(
                        rule,
                        target,
                        target.into(),
                        suffixes.stem(target),
                        Vec::new(),
                    )));
                }
            }
        }
        entries.extend(derived);
        makefile.entries = entries;
        Ok(suffixes)
    }

    fn is_inference(&self, rule: &Rule) -> bool {
        let [name] = rule.targets.as_slice() else {
            return false;
        };
        let Some(suffix) = name.strip_prefix('.').filter(|name| !name.contains('/')) else {
            return false;
        };
        match suffix.split_once('.') {
            Some((first, second)) => {
                !second.contains('.')
                    && self.known(&name[..first.len() + 1])
                    && self.known(&suffix[first.len()..])
            }
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
        // POSIX Libraries: .s2.a uses the member stem for the source and
        // applies only to archive-member targets.
        let (stem, suffix) = if let Some((_, member)) = archive_member(target) {
            if !self.known(".a") {
                return None;
            }
            (member.strip_suffix(".o")?, ".a")
        } else if let Some(parts) = self.suffix(target).filter(|&(_, suffix)| suffix != ".a") {
            parts
        } else if !has_rule
            && target
                .rsplit('/')
                .next()
                .is_some_and(|name| !name.contains('.'))
        {
            (target, "")
        } else {
            return None;
        };
        for source_suffix in &self.0 {
            let Some(rule) = rules.get(&format!("{source_suffix}{suffix}")) else {
                continue;
            };
            let source = format!("{stem}{source_suffix}");
            if source != target && Path::new(&source).exists() {
                let prerequisites = vec![source.clone()];
                return Some(derived_rule(rule, target, source, stem, prerequisites));
            }
        }
        None
    }
}

fn builtin_rules() -> HashMap<String, Rule> {
    let path = Path::new("<default rules>").into();
    DEFAULT_RULES
        .iter()
        .enumerate()
        .map(|(index, &(name, commands))| {
            let location = Location {
                path: std::sync::Arc::clone(&path),
                line: index + 1,
            };
            (
                name.into(),
                Rule {
                    location: location.clone(),
                    targets: vec![name.into()],
                    prerequisites: Vec::new(),
                    order_only: Vec::new(),
                    recipes: commands
                        .iter()
                        .map(|text| SourceLine {
                            location: location.clone(),
                            text: (*text).into(),
                        })
                        .collect(),
                    inference: None,
                },
            )
        })
        .collect()
}

fn derived_rule(
    rule: &Rule,
    target: &str,
    source: String,
    stem: &str,
    prerequisites: Vec<String>,
) -> Rule {
    Rule {
        location: rule.location.clone(),
        targets: vec![target.into()],
        prerequisites,
        order_only: Vec::new(),
        recipes: rule.recipes.clone(),
        inference: Some(Inference {
            source,
            stem: stem.into(),
        }),
    }
}
