//! Schedule recipes and check file freshness for POSIX.1-2024 make.

use crate::{
    graph::{archive_member, Graph, Node},
    inference::Suffixes,
    job_pool::{JobPool, Token},
    r#macro::{Automatic, Bindings, MacroSource, Macros, Values},
    reader::{Entry, IncludeFile, Makefile, SourceLine},
    signals,
    special::Specials,
    time, Error,
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fs,
    io::{self, IsTerminal, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, Command, ExitStatus},
    rc::Rc,
    thread,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Copy, Default)]
pub(crate) struct Options {
    pub dry_run: bool,
    pub keep_going: bool,
    pub silent: bool,
    pub ignore: bool,
    pub remaking: bool,
    pub jobs: Option<usize>,
}

/// Delayed remaking, as specified by POSIX.1-2024 make, "Include Lines".
/// Return the first changed include so the caller can read the new contents.
pub(crate) fn remake_includes<'a>(
    makefile: &'a Makefile,
    macros: &Macros,
    suffixes: &Suffixes,
    specials: &Specials,
    options: Options,
    pool: Option<&JobPool>,
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
        .filter(|include| {
            rule_targets.contains(include.name.as_str()) && !specials.phony.contains(&include.name)
        })
        .map(|include| include.name.clone())
        .collect();
    if !targets.is_empty() {
        let graph = Graph::for_targets(makefile, &targets).map_err(|error| error.to_string())?;
        let required: HashSet<_> = makefile
            .includes
            .iter()
            .filter(|include| !include.optional)
            .map(|include| include.name.as_str())
            .collect();
        // -n still executes the commands needed to remake includes. Failure
        // on an optional include does not stop the other include remakes.
        build(
            &graph,
            macros,
            suffixes,
            specials,
            Options {
                remaking: true,
                keep_going: true,
                ..options
            },
            pool,
            Some(&required),
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
    pool: Option<&JobPool>,
    required_includes: Option<&HashSet<&str>>,
) -> Result<(), String> {
    Build {
        graph,
        macros,
        suffixes,
        specials,
        options,
        // A phase supervisor can own the group for the whole build tree.
        own_group: !io::stdin().is_terminal()
            && std::env::var_os("BMAKE_INHERIT_PROCESS_GROUP").is_none_or(|value| value != "1"),
    }
    .run(pool, required_includes)
}

#[derive(Default)]
struct Pending {
    requested: bool,
    scope: Option<Rc<Bindings>>,
    child_scope: Option<Rc<Bindings>>,
    result: Option<bool>,
    parents: Vec<usize>,
    next_edge: usize,
    remaining: usize,
    failed: bool,
}

enum Event {
    Request(usize, Option<usize>),
    Advance(usize),
    PrerequisiteDone(usize, bool),
}

// A target requests the next group of its own prerequisites only once the
// current group succeeds. Another active rule can request the same targets
// independently. A .WAIT marker affects scheduling for its own rule.
struct Ready<'g, 'a> {
    graph: &'g Graph<'a>,
    macros: &'g Macros,
    pending: Vec<Pending>,
    ranks: Vec<usize>,
    ready: BinaryHeap<Reverse<usize>>,
    events: Vec<Event>,
}

impl<'g, 'a> Ready<'g, 'a> {
    fn new(graph: &'g Graph<'a>, macros: &'g Macros) -> Self {
        let mut ranks = vec![0; graph.nodes().len()];
        for (rank, &index) in graph.default_build_order().iter().enumerate() {
            ranks[index] = rank;
        }
        let events = graph
            .roots
            .iter()
            .rev()
            .map(|&index| Event::Request(index, None))
            .collect();
        Self {
            graph,
            macros,
            pending: (0..graph.nodes().len())
                .map(|_| Pending::default())
                .collect(),
            ranks,
            ready: BinaryHeap::new(),
            events,
        }
    }

    fn settle(&mut self) {
        while let Some(event) = self.events.pop() {
            match event {
                Event::Request(index, parent) => {
                    if let Some(parent) = parent {
                        if let Some(success) = self.pending[index].result {
                            self.events.push(Event::PrerequisiteDone(parent, success));
                            continue;
                        }
                        self.pending[index].parents.push(parent);
                    }
                    if !self.pending[index].requested {
                        let inherited =
                            parent.and_then(|parent| self.pending[parent].child_scope.as_ref());
                        let (scope, child_scope) = self
                            .macros
                            .target_scope(self.graph.nodes()[index].name, inherited);
                        let state = &mut self.pending[index];
                        state.scope = scope;
                        state.child_scope = child_scope;
                        state.requested = true;
                        self.events.push(Event::Advance(index));
                    }
                }
                Event::Advance(index) => self.advance(index),
                Event::PrerequisiteDone(parent, success) => {
                    let state = &mut self.pending[parent];
                    state.failed |= !success;
                    state.remaining -= 1;
                    if state.remaining == 0 {
                        if state.failed {
                            self.complete(parent, false);
                        } else {
                            self.events.push(Event::Advance(parent));
                        }
                    }
                }
            }
        }
    }

    fn advance(&mut self, index: usize) {
        let state = &mut self.pending[index];
        let edges = &self.graph.nodes()[index].prerequisites;
        while state.next_edge < edges.len() {
            let start = state.next_edge;
            let count = edges[start..]
                .iter()
                .take_while(|edge| edge.target.is_some())
                .count();
            state.next_edge = (start + count + 1).min(edges.len());
            if count > 0 {
                state.remaining = count;
                self.events.extend(
                    edges[start..start + count]
                        .iter()
                        .rev()
                        .filter_map(|edge| edge.target)
                        .map(|child| Event::Request(child, Some(index))),
                );
                return;
            }
        }
        self.ready.push(Reverse(self.ranks[index]));
    }

    fn complete(&mut self, index: usize, success: bool) {
        let state = &mut self.pending[index];
        state.result = Some(success);
        self.events.extend(
            state
                .parents
                .drain(..)
                .map(|parent| Event::PrerequisiteDone(parent, success)),
        );
    }

    fn pop(&mut self) -> Option<usize> {
        let Reverse(rank) = self.ready.pop()?;
        Some(self.graph.default_build_order()[rank])
    }
}

struct Build<'g, 'a> {
    graph: &'g Graph<'a>,
    macros: &'g Macros,
    suffixes: &'g Suffixes,
    specials: &'g Specials,
    options: Options,
    own_group: bool,
}

enum Update {
    Current(Option<SystemTime>),
    Recipes(Job),
}

struct Job {
    before: Option<SystemTime>,
    newer: String,
    order_only: String,
    scope: Option<Rc<Bindings>>,
    next_recipe: usize,
    skipped: bool,
}

struct Active {
    index: usize,
    job: Job,
    token: Option<Token>,
    recipe: RunningRecipe,
}

impl<'a> Build<'_, 'a> {
    fn run(
        &self,
        pool: Option<&JobPool>,
        required_includes: Option<&HashSet<&str>>,
    ) -> Result<(), String> {
        let limit = if self.specials.not_parallel {
            1
        } else {
            self.options.jobs.unwrap_or(1)
        };
        let mut ready = Ready::new(self.graph, self.macros);
        let mut mtimes = HashMap::new();
        let mut active: Vec<Active> = Vec::new();
        let mut errors = Vec::new();
        let mut interrupted = None;
        let mut stopped = false;
        let mut fatal = false;

        loop {
            if let Some(number) = signals::pending() {
                if interrupted.is_none() {
                    errors.push(format!("bmake: interrupted by signal {number}"));
                    interrupted = Some((number, Instant::now()));
                }
                stopped = true;
            }

            let mut i = 0;
            while i < active.len() {
                let recipe = &mut active[i].recipe;
                if let Some((number, since)) = interrupted {
                    recipe.interrupt(number, since.elapsed(), self.own_group);
                }
                let status = match recipe.child.try_wait() {
                    Ok(None) => {
                        i += 1;
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    status => status,
                };
                let mut work = active.swap_remove(i);
                let node = &self.graph.nodes()[work.index];
                let result = match work.recipe.finish(status) {
                    Ok(()) if !fatal => self.advance(node, &mut work.job),
                    Ok(()) => Ok(None),
                    Err(failure) => Err(failure),
                };
                fatal |= result.as_ref().is_err_and(|failure| failure.fatal);
                match result {
                    Ok(Some(recipe)) => {
                        work.recipe = recipe;
                        active.push(work);
                        // This is a new line for a target already in flight.
                        // Check the other children before polling it again.
                        i += 1;
                    }
                    finished => {
                        let result = self.finish(node, work.job, finished.map(|_| ()));
                        stopped |= fatal || (result.is_err() && !self.options.keep_going);
                        self.record(&mut ready, &mut mtimes, &mut errors, work.index, result);
                    }
                }
            }

            while !stopped && signals::pending().is_none() && active.len() < limit {
                ready.settle();
                if ready.ready.is_empty() {
                    break;
                }

                // The implicit slot is available as soon as its own job ends,
                // even if other local jobs still hold tokens.
                let token = if active.iter().any(|work| work.token.is_none()) {
                    match pool.expect("parallel build has a token pool").try_acquire() {
                        Ok(Some(token)) => Some(token),
                        Ok(None) => break,
                        Err(error) => {
                            errors.push(format!("job pool: {error}"));
                            stopped = true;
                            break;
                        }
                    }
                } else {
                    None
                };
                let index = ready.pop().expect("ready target");
                let node = &self.graph.nodes()[index];
                let result = match self.prepare(node, &mtimes, ready.pending[index].scope.clone()) {
                    Ok(Update::Recipes(mut job)) => match self.advance(node, &mut job) {
                        Ok(Some(recipe)) => {
                            active.push(Active {
                                index,
                                job,
                                token,
                                recipe,
                            });
                            continue;
                        }
                        finished => {
                            fatal |= finished.as_ref().is_err_and(|failure| failure.fatal);
                            self.finish(node, job, finished.map(|_| ()))
                        }
                    },
                    Ok(Update::Current(modified)) => Ok(modified),
                    Err(error) => Err(error),
                };
                stopped |= fatal || (result.is_err() && !self.options.keep_going);
                self.record(&mut ready, &mut mtimes, &mut errors, index, result);
            }

            if active.is_empty() {
                if interrupted.is_none() && signals::pending().is_some() {
                    continue;
                }
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }

        if errors.is_empty()
            || (!fatal && interrupted.is_none()
                && required_includes.is_some_and(|required| {
                    self.graph.roots.iter().all(|&index| {
                        let name = self.graph.nodes()[index].name;
                        !required.contains(name) || ready.pending[index].result == Some(true)
                    })
                }))
        {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn record(
        &self,
        ready: &mut Ready<'_, '_>,
        mtimes: &mut HashMap<&'a str, Option<SystemTime>>,
        errors: &mut Vec<String>,
        index: usize,
        result: Result<Option<SystemTime>, String>,
    ) {
        let success = result.is_ok();
        match result {
            Ok(modified) => {
                mtimes.insert(self.graph.nodes()[index].name, modified);
            }
            Err(error) => errors.push(error),
        }
        ready.complete(index, success);
    }

    fn prepare(
        &self,
        node: &Node<'_>,
        mtimes: &HashMap<&str, Option<SystemTime>>,
        scope: Option<Rc<Bindings>>,
    ) -> Result<Update, String> {
        let target = node.name;
        let modified = target_time(node, self.specials)?;
        if modified.is_none() && !self.specials.phony.contains(target) && !node.has_rule {
            return Err(format!("no rule to make prerequisite '{target}'"));
        }
        if node.recipes().is_empty() {
            return Ok(Update::Current(modified));
        }
        let newer: Vec<_> = self
            .graph
            .normal_prerequisites(node)
            .filter(|name| time::newer(modified, mtimes[name]))
            .collect();
        if modified.is_some() && newer.is_empty() {
            return Ok(Update::Current(modified));
        }
        let mut seen = HashSet::new();
        let newer = newer
            .into_iter()
            .filter(|name| seen.insert(*name))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(Update::Recipes(Job {
            before: modified,
            newer,
            order_only: self.graph.order_only_prerequisites(node),
            scope,
            next_recipe: 0,
            skipped: false,
        }))
    }

    fn advance(
        &self,
        node: &Node<'_>,
        job: &mut Job,
    ) -> Result<Option<RunningRecipe>, RecipeFailure> {
        if signals::pending().is_some() {
            return Ok(None);
        }
        let target = node.name;
        let inference = node.inference();
        let mut automatic = Automatic::for_target(target);
        let prerequisites: Vec<_> = self.graph.normal_prerequisites(node).collect();
        automatic.source = inference
            .map(|rule| rule.source.as_str())
            .or_else(|| prerequisites.first().copied());
        automatic.prerequisites = &prerequisites;
        automatic.stem = inference.map_or_else(
            || self.suffixes.stem(automatic.member.unwrap_or(target)),
            |rule| &rule.stem,
        );
        automatic.newer = &job.newer;
        automatic.order_only = &job.order_only;

        let mode = RecipeMode {
            dry_run: self.options.dry_run,
            remaking: self.options.remaking,
            ignore: self.options.ignore || self.specials.ignore.contains(target),
            silent: self.options.silent || self.specials.silent.contains(target),
        };
        while let Some(recipe) = node.recipes().get(job.next_recipe) {
            job.next_recipe += 1;
            match start_recipe(
                target,
                recipe,
                self.macros.view(job.scope.as_deref()),
                &automatic,
                mode,
                self.own_group,
            )? {
                Recipe::Empty => (),
                Recipe::Skipped => job.skipped = true,
                Recipe::Running(running) => return Ok(Some(running)),
            }
        }
        Ok(None)
    }

    fn finish(
        &self,
        node: &Node<'_>,
        job: Job,
        result: Result<(), RecipeFailure>,
    ) -> Result<Option<SystemTime>, String> {
        let interrupted = signals::pending().is_some();
        let child_interrupted = result.as_ref().is_err_and(|failure| failure.interrupted);
        let target = node.name;
        let cleanup = if ((interrupted && !self.options.dry_run)
            || (child_interrupted && (!self.options.dry_run || self.options.remaking)))
            && job.next_recipe != 0
            && !self.specials.phony.contains(target)
            && archive_member(target).is_none()
            && !self.specials.precious.contains(target)
        {
            remove_interrupted_target(target, job.before)
        } else {
            Ok(())
        };
        match (result, cleanup) {
            (Err(failure), Err(cleanup)) => Err(format!("{}\n{cleanup}", failure.message)),
            (Err(failure), Ok(())) => Err(failure.message),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Ok(()), Ok(())) if job.skipped || interrupted => Ok(None),
            (Ok(()), Ok(())) => target_time(node, self.specials),
        }
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
    fatal: bool,
}

enum Recipe {
    Empty,
    Skipped,
    Running(RunningRecipe),
}

struct RunningRecipe {
    child: Child,
    location: String,
    ignore: bool,
    sent_interrupt: bool,
    sent_kill: bool,
}

impl RunningRecipe {
    fn interrupt(&mut self, number: i32, elapsed: Duration, own_group: bool) {
        let force = elapsed >= Duration::from_secs(1);
        let sent = if force {
            &mut self.sent_kill
        } else {
            &mut self.sent_interrupt
        };
        if !*sent {
            // A shell that exited since the last poll no longer needs the signal.
            let _ = signals::interrupt_child(self.child.id(), own_group, number, force);
            *sent = true;
        }
    }

    fn finish(&self, status: io::Result<Option<ExitStatus>>) -> Result<(), RecipeFailure> {
        let (reason, interrupted) = match status {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => match (status.code(), status.signal()) {
                (Some(code), _) => (format!("recipe exited with status {code}"), false),
                (_, Some(signal)) => (
                    format!("recipe terminated by signal {signal}"),
                    signals::interrupts_recipe(signal),
                ),
                _ => (format!("recipe failed: {status}"), false),
            },
            Ok(None) => unreachable!("running recipe is not finished"),
            Err(error) => (format!("could not wait for /bin/sh: {error}"), false),
        };
        let message = format!("{}: {reason}", self.location);
        if self.ignore {
            let _ = writeln!(io::stderr(), "{message} (ignored)");
            Ok(())
        } else {
            Err(RecipeFailure {
                message,
                interrupted,
                fatal: false,
            })
        }
    }
}

fn start_recipe(
    target: &str,
    recipe: &SourceLine,
    macros: Values<'_>,
    automatic: &Automatic<'_>,
    mode: RecipeMode,
    own_group: bool,
) -> Result<Recipe, RecipeFailure> {
    let (expanded, expanded_make) = macros
        .expand_recipe(&recipe.text, &recipe.location, automatic)
        .map_err(|error| RecipeFailure {
            message: error.to_string(),
            interrupted: false,
            fatal: true,
        })?;
    let line = expanded.trim_start_matches([' ', '\t', '-', '@', '+']);
    let prefixes = &expanded[..expanded.len() - line.len()];
    if line.is_empty() {
        return Ok(Recipe::Empty);
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
                fatal: false,
            })?;
    }
    if skip {
        return Ok(Recipe::Skipped);
    }

    let ignore = mode.ignore || prefixes.contains('-');
    let shell_flag = if ignore { "-c" } else { "-ec" };
    let mut shell = Command::new("/bin/sh");
    macros
        .set_recipe_environment(&mut shell, &recipe.location, automatic)
        .map_err(|error| RecipeFailure {
            message: error.to_string(),
            interrupted: false,
            fatal: true,
        })?;
    if own_group {
        shell.process_group(0);
    }
    let child = match shell.args([shell_flag, line]).spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = diagnostic(format!("could not run /bin/sh: {error}"));
            if ignore {
                let _ = writeln!(io::stderr(), "{message} (ignored)");
                return Ok(Recipe::Empty);
            }
            return Err(RecipeFailure {
                message,
                interrupted: false,
                fatal: false,
            });
        }
    };
    Ok(Recipe::Running(RunningRecipe {
        child,
        location: format!("{}: target '{target}'", recipe.location),
        ignore,
        sent_interrupt: false,
        sent_kill: false,
    }))
}
