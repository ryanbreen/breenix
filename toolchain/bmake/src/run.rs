//! Serial recipes and file freshness for POSIX.1-2024 make.

use crate::{
    graph::{archive_member, Graph, Node},
    inference::Suffixes,
    r#macro::{Automatic, MacroSource, Macros},
    reader::{Entry, IncludeFile, Makefile, SourceLine},
    special::Specials,
    time, Error,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    path::Path,
    process::Command,
    time::SystemTime,
};

#[derive(Clone, Copy, Default)]
pub(crate) struct Options {
    pub dry_run: bool,
    pub keep_going: bool,
    pub silent: bool,
    pub ignore: bool,
    pub remaking: bool,
}

/// Delayed remaking, as specified by POSIX.1-2024 make, "Include Lines".
/// Return the first changed include so the caller can read the new contents.
pub(crate) fn remake_includes<'a>(
    makefile: &'a Makefile,
    macros: &Macros,
    suffixes: &Suffixes,
    specials: &Specials,
    options: Options,
) -> Result<Option<&'a IncludeFile>, String> {
    if makefile.includes.is_empty() {
        return Ok(None);
    }

    let mut rule_targets = HashSet::new();
    for entry in &makefile.entries {
        if let Entry::Rule(rule) = entry {
            rule_targets.extend(rule.targets.iter().map(String::as_str));
        }
    }
    let targets: Vec<_> = makefile
        .includes
        .iter()
        .filter(|include| rule_targets.contains(include.name.as_str()))
        .map(|include| include.name.clone())
        .collect();
    if !targets.is_empty() {
        let graph = Graph::for_targets(makefile, &targets).map_err(|error| error.to_string())?;
        // -n still executes the commands needed to remake includes.
        build(
            &graph,
            macros,
            suffixes,
            specials,
            Options {
                remaking: true,
                ..options
            },
        )?;
        for include in &makefile.includes {
            let current = fs::metadata(&include.name).and_then(|metadata| {
                if metadata.is_file() {
                    fs::read(&include.name).map(Some)
                } else {
                    // Do not reopen a stream to check for changes.
                    Ok(None)
                }
            });
            let changed = match current {
                Ok(Some(contents)) => include.contents.as_deref() != Some(&contents),
                Ok(None) => false,
                Err(error) if error.kind() == io::ErrorKind::NotFound || include.optional => {
                    include.contents.is_some()
                }
                Err(error) => {
                    return Err(Error::new(
                        &include.location,
                        format!("include '{}': {error}", include.name),
                    )
                    .to_string())
                }
            };
            if changed {
                return Ok(Some(include));
            }
        }
    }
    for include in &makefile.includes {
        if include.contents.is_none() && !include.optional {
            return Err(Error::new(
                &include.location,
                format!("include '{}': file not found", include.name),
            )
            .to_string());
        }
    }
    Ok(None)
}

pub(crate) fn build(
    graph: &Graph<'_>,
    macros: &Macros,
    suffixes: &Suffixes,
    specials: &Specials,
    options: Options,
) -> Result<(), String> {
    let mut mtimes = HashMap::new();
    let mut errors = Vec::new();
    for node in graph.default_build_order() {
        // A failed or skipped prerequisite has no entry. Its dependents cannot run.
        if graph
            .prerequisites_of(node)
            .any(|name| !mtimes.contains_key(name))
        {
            continue;
        }
        match update(node, graph, &mtimes, macros, suffixes, specials, &options) {
            Ok(modified) => {
                mtimes.insert(node.name, modified);
            }
            Err(error) if options.keep_going => errors.push(error),
            Err(error) => return Err(error),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn target_time(node: &Node<'_>, specials: &Specials) -> Result<Option<SystemTime>, String> {
    let target = node.name;
    if specials.phony.contains(target) {
        Ok(None)
    } else if let Some((archive, member)) = archive_member(target) {
        time::member_modified(archive, member).map_err(|reason| {
            let location = node
                .location
                .cloned()
                .unwrap_or_else(|| MacroSource::CommandLine.location());
            Error::new(&location, format!("target '{target}': {reason}")).to_string()
        })
    } else {
        time::modified(target)
    }
}

fn update(
    node: &Node<'_>,
    graph: &Graph<'_>,
    mtimes: &HashMap<&str, Option<SystemTime>>,
    macros: &Macros,
    suffixes: &Suffixes,
    specials: &Specials,
    options: &Options,
) -> Result<Option<SystemTime>, String> {
    let target = node.name;
    let phony = specials.phony.contains(target);
    let modified = target_time(node, specials)?;
    if modified.is_none() && !phony && !node.has_rule {
        return Err(format!("no rule to make prerequisite '{target}'"));
    }
    if node.recipes().is_empty() {
        return Ok(modified);
    }
    let newer: Vec<_> = graph
        .prerequisites_of(node)
        .filter(|name| time::newer(modified, mtimes[name]))
        .collect();
    if modified.is_some() && newer.is_empty() {
        return Ok(modified);
    }

    let inference = node.inference();
    let mut automatic = Automatic::for_target(target);
    automatic.source = inference.map(|rule| rule.source.as_str());
    automatic.stem = inference.map_or_else(
        || suffixes.stem(automatic.member.unwrap_or(target)),
        |rule| &rule.stem,
    );
    let newer = newer.join(" ");
    automatic.newer = &newer;

    let mode = RecipeMode {
        dry_run: options.dry_run,
        remaking: options.remaking,
        ignore: options.ignore || specials.ignore.contains(target),
        silent: options.silent || specials.silent.contains(target),
    };
    let mut skipped = false;
    for recipe in node.recipes() {
        match run_recipe(target, recipe, macros, &automatic, mode) {
            Ok(command_skipped) => skipped |= command_skipped,
            Err(failure) => {
                if failure.interrupted
                    && (!mode.dry_run || mode.remaking)
                    && !phony
                    && automatic.member.is_none()
                    && !specials.precious.contains(target)
                {
                    if let Err(cleanup) = remove_interrupted_target(target, modified) {
                        return Err(format!("{}\n{cleanup}", failure.message));
                    }
                }
                return Err(failure.message);
            }
        }
    }
    if skipped {
        Ok(None)
    } else {
        target_time(node, specials)
    }
}

fn remove_interrupted_target(target: &str, before: Option<SystemTime>) -> Result<(), String> {
    let after = time::modified(target)?;
    if after.is_some() && after != before && !Path::new(target).is_dir() {
        fs::remove_file(target).map_err(|error| format!("could not remove '{target}': {error}"))?;
        let _ = writeln!(io::stderr(), "removed interrupted target '{target}'");
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RecipeMode {
    dry_run: bool,
    remaking: bool,
    ignore: bool,
    silent: bool,
}

struct RecipeFailure {
    message: String,
    interrupted: bool,
}

fn run_recipe(
    target: &str,
    recipe: &SourceLine,
    macros: &Macros,
    automatic: &Automatic<'_>,
    mode: RecipeMode,
) -> Result<bool, RecipeFailure> {
    let (expanded, expanded_make) = macros
        .expand_recipe(&recipe.text, &recipe.location, automatic)
        .map_err(|error| RecipeFailure {
            message: error.to_string(),
            interrupted: false,
        })?;
    let line = expanded.trim_start_matches([' ', '\t', '-', '@', '+']);
    let prefixes = &expanded[..expanded.len() - line.len()];
    if line.is_empty() {
        return Ok(false);
    }
    let skip = mode.dry_run && !mode.remaking && !expanded_make && !prefixes.contains('+');
    let diagnostic = |reason| format!("{}: target '{target}': {reason}", recipe.location);

    if skip || (!mode.silent && (mode.dry_run || !prefixes.contains('@'))) {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|error| RecipeFailure {
                message: diagnostic(format!("could not echo recipe: {error}")),
                interrupted: false,
            })?;
    }
    if skip {
        return Ok(true);
    }

    let ignore = mode.ignore || prefixes.contains('-');
    let shell_flag = if ignore { "-c" } else { "-ec" };
    let mut shell = Command::new("/bin/sh");
    macros
        .set_recipe_environment(&mut shell, &recipe.location)
        .map_err(|error| RecipeFailure {
            message: error.to_string(),
            interrupted: false,
        })?;
    let (reason, interrupted) = match shell.args([shell_flag, line]).status() {
        Ok(status) if status.success() => return Ok(false),
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => (format!("recipe exited with status {code}"), false),
            (_, Some(signal)) => (
                format!("recipe terminated by signal {signal}"),
                // SIGHUP, SIGINT, SIGQUIT and SIGTERM on the supported Unix hosts.
                matches!(signal, 1 | 2 | 3 | 15),
            ),
            _ => (format!("recipe failed: {status}"), false),
        },
        Err(error) => (format!("could not run /bin/sh: {error}"), false),
    };
    let message = diagnostic(reason);
    if ignore {
        let _ = writeln!(io::stderr(), "{message} (ignored)");
        Ok(false)
    } else {
        Err(RecipeFailure {
            message,
            interrupted,
        })
    }
}
