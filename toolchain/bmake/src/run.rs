//! Serial recipe execution using POSIX.1-2024 make's "Makefile Execution" prefixes.

use crate::{
    graph::Graph,
    inference::{archive_parts, Suffixes},
    r#macro::{Automatic, Macros},
    reader::SourceLine,
    time,
};
use std::{
    collections::HashMap,
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    process::Command,
};

/// Update the selected targets and their prerequisites in the current directory.
pub(crate) fn build(graph: &Graph<'_>, macros: &Macros, suffixes: &Suffixes) -> Result<(), String> {
    let mut mtimes = HashMap::new();
    for node in graph.default_build_order() {
        let target = node.name;
        let (filename, member) = archive_parts(target);
        // Archive member timestamps are handled by 25.6c. For now an archive
        // target can run an explicit recipe with the member-name macros.
        let modified_time = || {
            if member.is_some() {
                Ok(None)
            } else {
                time::modified(filename)
            }
        };
        let mut modified = modified_time()?;
        if modified.is_none() && !node.has_rule {
            return Err(format!("no rule to make prerequisite '{target}'"));
        }

        // The graph visits prerequisites first, including shared prerequisites once.
        if !node.recipes().is_empty() {
            let newer: Vec<_> = graph
                .prerequisites_of(node)
                .filter(|name| time::newer(modified, mtimes[name]))
                .collect();
            if modified.is_none() || !newer.is_empty() {
                let inference = node.inference();
                let source = inference.map(|rule| rule.source.as_str());
                let newer = match (member, source) {
                    (Some(_), Some(source)) => source.to_owned(),
                    _ => newer.join(" "),
                };
                let automatic = Automatic {
                    target: filename,
                    source,
                    newer: &newer,
                    stem: inference.map_or_else(
                        || suffixes.stem(member.unwrap_or(filename)),
                        |rule| &rule.stem,
                    ),
                    member,
                };
                for recipe in node.recipes() {
                    run_recipe(target, recipe, macros, &automatic)?;
                }
                modified = modified_time()?;
            }
        }
        mtimes.insert(target, modified);
    }
    Ok(())
}

fn run_recipe(
    target: &str,
    recipe: &SourceLine,
    macros: &Macros,
    automatic: &Automatic<'_>,
) -> Result<(), String> {
    let expanded = macros
        .expand(&recipe.text, &recipe.location, automatic)
        .map_err(|error| error.to_string())?;
    let line = expanded.trim_start_matches([' ', '\t', '-', '@']);
    let prefixes = &expanded[..expanded.len() - line.len()];
    if line.is_empty() {
        return Ok(());
    }
    let diagnostic = |reason| format!("{}: target '{target}': {reason}", recipe.location);

    if !prefixes.contains('@') {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|error| diagnostic(format!("could not echo recipe: {error}")))?;
    }

    let reason = match Command::new("/bin/sh")
        .args(["-c", line])
        .envs(macros.recipe_environment())
        .status()
    {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => format!("recipe exited with status {code}"),
            (_, Some(signal)) => format!("recipe terminated by signal {signal}"),
            _ => format!("recipe failed: {status}"),
        },
        Err(error) => format!("could not run /bin/sh: {error}"),
    };
    let message = diagnostic(reason);
    if prefixes.contains('-') {
        let _ = writeln!(io::stderr(), "{message} (ignored)");
        Ok(())
    } else {
        Err(message)
    }
}
