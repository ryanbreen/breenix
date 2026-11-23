//! POSIX.1-2024 suffix inference and GNU make 4.4.1 §§4.11, 10.5 and 10.8.

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

        let mut suffix_rules = builtin_rules();
        let mut explicit_suffixes = HashSet::new();
        let mut pattern_rules: Vec<PatternRule> = Vec::new();
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
                    explicit_suffixes.insert(rule.targets[0].clone());
                    suffix_rules.insert(rule.targets[0].clone(), rule);
                }
                Entry::Rule(mut rule)
                    if rule.inference.is_none()
                        && rule.targets.iter().any(|name| name.contains('%')) =>
                {
                    let patterns: Vec<_> = rule
                        .targets
                        .iter()
                        .map(|name| Pattern::parse(name))
                        .collect();
                    if patterns.iter().any(Pattern::has_stem) {
                        let [target] = patterns.as_slice() else {
                            return Err(Error::new(
                                &rule.location,
                                "grouped implicit pattern targets are unsupported",
                            ));
                        };
                        pattern_rules.retain(|previous| {
                            previous.target != *target
                                || previous.rule.prerequisites != rule.prerequisites
                                || previous.rule.order_only != rule.order_only
                                || previous.rule.terminal != rule.terminal
                        });
                        pattern_rules.push(PatternRule {
                            target: target.clone(),
                            rule,
                        });
                    } else {
                        if rule.terminal {
                            return Err(Error::new(
                                &rule.location,
                                "double-colon rules require an implicit pattern target",
                            ));
                        }
                        rule.targets = patterns.into_iter().map(|pattern| pattern.prefix).collect();
                        entries.push(Entry::Rule(rule));
                    }
                }
                Entry::Rule(rule) if rule.terminal => {
                    return Err(Error::new(
                        &rule.location,
                        "double-colon rules require an implicit pattern target",
                    ));
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
        let mentioned = candidates.clone();
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
        let search = ImplicitSearch {
            suffixes: &suffixes,
            suffix_rules: &suffix_rules,
            explicit_suffixes: &explicit_suffixes,
            patterns: &pattern_rules,
            targets: &targets,
            mentioned: &mentioned,
        };
        let mut derived = Vec::new();
        let mut selected = HashSet::new();
        for target in candidates {
            if target == ".WAIT" {
                continue;
            }
            let has_recipe = targets.get(target).copied();
            if has_recipe == Some(true)
                || specials.phony.contains(target)
                || selected.contains(target)
            {
                continue;
            }
            let mut plan = Vec::new();
            if search.infer(target, &selected, &mut Vec::new(), &mut plan) {
                for rule in plan {
                    selected.insert(rule.targets[0].clone());
                    derived.push(Entry::Rule(rule));
                }
            } else if let Some(rule) = &fallback {
                if has_recipe.is_none() && !Path::new(target).exists() {
                    derived.push(Entry::Rule(derived_rule(
                        rule,
                        target,
                        target.into(),
                        suffixes.stem(target).into(),
                        Vec::new(),
                        Vec::new(),
                        true,
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

    fn infer(
        &self,
        target: &str,
        has_rule: bool,
        rules: &HashMap<String, Rule>,
        explicit: &HashSet<String>,
    ) -> Option<(Rule, bool)> {
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
            let name = format!("{source_suffix}{suffix}");
            let Some(rule) = rules.get(&name) else {
                continue;
            };
            let source = format!("{stem}{source_suffix}");
            if source != target && Path::new(&source).exists() {
                let prerequisites = vec![source.clone()];
                let rule = derived_rule(
                    rule,
                    target,
                    source,
                    stem.into(),
                    prerequisites,
                    Vec::new(),
                    true,
                );
                return Some((rule, explicit.contains(&name)));
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
                    pattern: None,
                    terminal: false,
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
    stem: String,
    prerequisites: Vec<String>,
    order_only: Vec<String>,
    implicit: bool,
) -> Rule {
    Rule {
        location: rule.location.clone(),
        targets: vec![target.into()],
        prerequisites,
        order_only,
        recipes: rule.recipes.clone(),
        pattern: None,
        terminal: false,
        inference: Some(Inference {
            source,
            stem,
            implicit,
        }),
    }
}

/// The first unquoted percent separates the fixed parts of a GNU pattern.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Pattern {
    prefix: String,
    suffix: Option<String>,
}

impl Pattern {
    pub(crate) fn parse(mut text: &str) -> Self {
        let mut prefix = String::new();
        while let Some((before, after)) = text.split_once('%') {
            let literal = before.trim_end_matches('\\');
            let slashes = before.len() - literal.len();
            prefix.push_str(literal);
            prefix.extend(std::iter::repeat_n('\\', slashes / 2));
            if slashes % 2 == 0 {
                return Self {
                    prefix,
                    suffix: Some(after.into()),
                };
            }
            prefix.push('%');
            text = after;
        }
        prefix.push_str(text);
        Self {
            prefix,
            suffix: None,
        }
    }

    pub(crate) fn has_stem(&self) -> bool {
        self.suffix.is_some()
    }

    pub(crate) fn stem<'a>(&self, name: &'a str) -> Option<&'a str> {
        match &self.suffix {
            Some(suffix) => name.strip_prefix(&self.prefix)?.strip_suffix(suffix),
            None => (name == self.prefix).then_some(""),
        }
    }

    pub(crate) fn fill(&self, directory: &str, stem: &str) -> String {
        match &self.suffix {
            Some(suffix) => format!("{directory}{}{stem}{suffix}", self.prefix),
            None => self.prefix.clone(),
        }
    }

    fn has_slash(&self) -> bool {
        self.prefix.contains('/') || self.suffix.as_ref().is_some_and(|part| part.contains('/'))
    }

    fn match_anything(&self) -> bool {
        self.prefix.is_empty() && self.suffix.as_deref() == Some("")
    }
}

pub(crate) fn static_rules(rule: Rule, pattern: &str) -> Result<impl Iterator<Item = Rule>, Error> {
    let target_pattern = Pattern::parse(pattern);
    if rule.terminal || !target_pattern.has_stem() || pattern.split_ascii_whitespace().count() != 1
    {
        return Err(Error::new(
            &rule.location,
            "expected one static target pattern containing '%'",
        ));
    }
    Ok((0..rule.targets.len()).map(move |index| {
        let target = &rule.targets[index];
        if let Some(stem) = target_pattern.stem(target) {
            from_pattern(&rule, target, "", stem, false)
        } else {
            eprintln!(
                "{}: warning: target '{target}' does not match target pattern",
                rule.location
            );
            derived_rule(
                &rule,
                target,
                String::new(),
                String::new(),
                Vec::new(),
                Vec::new(),
                false,
            )
        }
    }))
}

fn from_pattern(rule: &Rule, target: &str, directory: &str, stem: &str, implicit: bool) -> Rule {
    let expand = |name: &String| Pattern::parse(name).fill(directory, stem);
    let prerequisites: Vec<_> = rule.prerequisites.iter().map(expand).collect();
    let order_only = rule.order_only.iter().map(expand).collect();
    let source = prerequisites
        .iter()
        .find(|name| name.as_str() != ".WAIT")
        .cloned()
        .unwrap_or_default();
    derived_rule(
        rule,
        target,
        source,
        format!("{directory}{stem}"),
        prerequisites,
        order_only,
        implicit,
    )
}

struct PatternRule {
    target: Pattern,
    rule: Rule,
}

struct ImplicitSearch<'a> {
    suffixes: &'a Suffixes,
    suffix_rules: &'a HashMap<String, Rule>,
    explicit_suffixes: &'a HashSet<String>,
    patterns: &'a [PatternRule],
    targets: &'a HashMap<&'a str, bool>,
    mentioned: &'a HashSet<&'a str>,
}

impl ImplicitSearch<'_> {
    fn infer(
        &self,
        target: &str,
        selected: &HashSet<String>,
        active: &mut Vec<(String, usize)>,
        plan: &mut Vec<Rule>,
    ) -> bool {
        let has_rule = self.targets.contains_key(target);
        let suffix =
            self.suffixes
                .infer(target, has_rule, self.suffix_rules, self.explicit_suffixes);
        // Preserve a caller's applicable POSIX inference rule. GNU patterns
        // can override the built-in fallback.
        if !suffix.as_ref().is_some_and(|(_, explicit)| *explicit)
            && self.patterns(target, selected, active, plan)
        {
            return true;
        }
        if let Some((rule, _)) = suffix {
            plan.push(rule);
            return true;
        }
        false
    }

    fn patterns(
        &self,
        target: &str,
        selected: &HashSet<String>,
        active: &mut Vec<(String, usize)>,
        plan: &mut Vec<Rule>,
    ) -> bool {
        let (directory, file) = target
            .rsplit_once('/')
            .map_or(("", target), |(dir, name)| (&target[..=dir.len()], name));
        let mut matches: Vec<_> = self
            .patterns
            .iter()
            .enumerate()
            .filter_map(|(index, rule)| {
                let (directory, name) = if rule.target.has_slash() {
                    ("", target)
                } else {
                    (directory, file)
                };
                let stem = rule.target.stem(name).filter(|stem| !stem.is_empty())?;
                Some((index, rule, directory, stem))
            })
            .collect();
        let specific = matches
            .iter()
            .any(|(_, rule, _, _)| !rule.target.match_anything());
        matches.retain(|(index, rule, _, _)| {
            !rule.rule.recipes.is_empty()
                && (rule.rule.terminal
                    || !rule.target.match_anything()
                    || (!specific && active.is_empty()))
                && !active.iter().any(|(_, ancestor)| index == ancestor)
        });
        matches.sort_by_key(|(_, _, directory, stem)| directory.len() + stem.len());

        // Try rules with available prerequisites before recursively looking
        // for ways to make intermediate files. Roll back rejected chains.
        for chain in [false, true] {
            for &(index, rule, directory, stem) in &matches {
                if chain && rule.rule.terminal {
                    continue;
                }
                let expanded = from_pattern(&rule.rule, target, directory, stem, true);
                let checkpoint = plan.len();
                let ready = expanded
                    .prerequisites
                    .iter()
                    .chain(&expanded.order_only)
                    .all(|name| {
                        if name == target || active.iter().any(|(ancestor, _)| name == ancestor) {
                            return false;
                        }
                        if name == ".WAIT"
                            || self.mentioned.contains(name.as_str())
                            || Path::new(name).exists()
                            || selected.contains(name)
                            || plan.iter().any(|rule| rule.targets[0] == *name)
                        {
                            return true;
                        }
                        if !chain {
                            return false;
                        }
                        active.push((target.into(), index));
                        let ready = self.infer(name, selected, active, plan);
                        active.pop();
                        ready
                    });
                if ready {
                    plan.push(expanded);
                    return true;
                }
                plan.truncate(checkpoint);
            }
        }
        false
    }
}
