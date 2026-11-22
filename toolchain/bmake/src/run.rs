//! Serial recipes and file freshness for POSIX.1-2024 make.

use crate::{
    graph::{Graph, Node},
    inference::Suffixes,
    r#macro::{Automatic, Macros},
    reader::{IncludeFile, SourceLine},
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

#[derive(Default)]
pub(crate) struct Options {
    pub dry_run: bool,
    pub keep_going: bool,
    pub silent: bool,
    pub ignore: bool,
}

/// Flag includes that would need an update, without executing any recipe.
pub(crate) fn check_includes(
    graph: &Graph<'_>,
    includes: &[IncludeFile],
    specials: &Specials,
) -> Result<(), String> {
    let mut mtimes = HashMap::new();
    let mut would_run = HashSet::new();
    for node in graph.default_build_order() {
        let modified = target_time(node.name, specials)?;
        let runs_recipe = !node.recipes().is_empty()
            && (modified.is_none()
                || graph
                    .prerequisites_of(node)
                    .any(|name| time::newer(modified, mtimes[name])));
        if runs_recipe
            || graph
                .prerequisites_of(node)
                .any(|name| would_run.contains(name))
        {
            would_run.insert(node.name);
        }
        mtimes.insert(node.name, if runs_recipe { None } else { modified });
    }
    for include in includes {
        if would_run.contains(include.name.as_str()) {
            return Err(Error::deferred(
                &include.location,
                &format!("remaking include '{}'", include.name),
            )
            .to_string());
        }
        if include.missing && !include.optional {
            return Err(Error::new(
                &include.location,
                format!("include '{}': file not found", include.name),
            )
            .to_string());
        }
    }
    Ok(())
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

fn target_time(target: &str, specials: &Specials) -> Result<Option<SystemTime>, String> {
    if specials.phony.contains(target) {
        Ok(None)
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
    let modified = target_time(target, specials)?;
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
    automatic.stem = inference.map_or_else(|| suffixes.stem(target), |rule| &rule.stem);
    let newer = newer.join(" ");
    automatic.newer = &newer;

    let mode = RecipeMode {
        dry_run: options.dry_run,
        ignore: options.ignore || specials.ignore.contains(target),
        silent: options.silent || specials.silent.contains(target),
    };
    let mut skipped = false;
    for recipe in node.recipes() {
        match run_recipe(target, recipe, macros, &automatic, mode) {
            Ok(command_skipped) => skipped |= command_skipped,
            Err(failure) => {
                if failure.interrupted
                    && !mode.dry_run
                    && !phony
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
        target_time(target, specials)
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
    let expanded = macros
        .expand(&recipe.text, &recipe.location, automatic)
        .map_err(|error| RecipeFailure {
            message: error.to_string(),
            interrupted: false,
        })?;
    let line = expanded.trim_start_matches([' ', '\t', '-', '@', '+']);
    let prefixes = &expanded[..expanded.len() - line.len()];
    if line.is_empty() {
        return Ok(false);
    }
    let skip = mode.dry_run && !prefixes.contains('+');
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
    let (reason, interrupted) = match Command::new("/bin/sh")
        .args([shell_flag, line])
        .envs(macros.recipe_environment())
        .status()
    {
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
