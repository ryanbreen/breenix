//! Serial recipe execution using POSIX.1-2024 make's "Makefile Execution" prefixes.

use crate::{graph::Graph, reader::SourceLine, time};
use std::{
    collections::HashMap,
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    process::Command,
};

/// Update the first declared target and its prerequisites in the current directory.
pub fn build(graph: &Graph<'_>) -> Result<(), String> {
    let mut mtimes = HashMap::new();
    for node in graph.default_build_order() {
        let target = node.name;
        let mut modified = time::modified(target)?;
        if modified.is_none() && !node.has_rule {
            return Err(format!("no rule to make prerequisite '{target}'"));
        }

        // The graph visits prerequisites first, including shared prerequisites once.
        let prerequisites = graph.prerequisites_of(node).map(|name| mtimes[name]);
        if !node.recipes().is_empty() && time::out_of_date(modified, prerequisites) {
            for recipe in node.recipes() {
                run_recipe(target, recipe)?;
            }
            modified = time::modified(target)?;
        }
        mtimes.insert(target, modified);
    }
    Ok(())
}

fn run_recipe(target: &str, recipe: &SourceLine) -> Result<(), String> {
    let line = recipe.text.trim_start_matches([' ', '\t', '-', '@']);
    let prefixes = &recipe.text[..recipe.text.len() - line.len()];
    let diagnostic = |reason| format!("{}: target '{target}': {reason}", recipe.location);

    if !prefixes.contains('@') {
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|error| diagnostic(format!("could not echo recipe: {error}")))?;
    }

    let reason = match Command::new("/bin/sh").args(["-c", line]).status() {
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
