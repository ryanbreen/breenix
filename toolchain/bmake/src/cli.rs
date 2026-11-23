use crate::{
    graph::Graph,
    inference::Suffixes,
    r#macro::{MacroSource, Macros},
    reader, run,
    special::Specials,
    Error,
};
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Options {
    environment_overrides: bool,
    execution: run::Options,
    stop_on_error: bool,
    makefiles: Vec<PathBuf>,
    assignments: Vec<String>,
    targets: Vec<String>,
}

impl Options {
    fn parse(
        &mut self,
        arguments: impl IntoIterator<Item = OsString>,
        source: MacroSource,
    ) -> Result<(), String> {
        let location = source.location();
        let mut arguments = arguments.into_iter();
        let mut parse_options = true;
        while let Some(argument) = arguments.next() {
            let text = argument.to_str().ok_or_else(|| {
                format!("invalid UTF-8 argument '{}'", argument.to_string_lossy())
            })?;
            if parse_options {
                if text == "--" {
                    parse_options = false;
                    continue;
                }
                if let Some(flags) = text.strip_prefix('-').filter(|flags| !flags.is_empty()) {
                    for (index, flag) in flags.char_indices() {
                        match flag {
                            'e' => self.environment_overrides = true,
                            'n' => self.execution.dry_run = true,
                            'k' | 'S' => {
                                self.execution.keep_going = flag == 'k';
                                self.stop_on_error = flag == 'S';
                            }
                            's' => self.execution.silent = true,
                            'i' => self.execution.ignore = true,
                            'j' => return Err(Error::deferred(&location, "-j").to_string()),
                            'f' | 'C' if source == MacroSource::CommandLine => {
                                let tail = &flags[index + 1..];
                                let path = if tail.is_empty() {
                                    arguments
                                        .next()
                                        .ok_or_else(|| format!("-{flag} requires a path"))?
                                } else {
                                    OsString::from(tail)
                                };
                                if flag == 'C' {
                                    env::set_current_dir(&path).map_err(|error| {
                                        format!("{}: {error}", Path::new(&path).display())
                                    })?;
                                } else {
                                    self.makefiles.push(path.into());
                                }
                                break;
                            }
                            _ => return Err(format!("unknown option '-{flag}'")),
                        }
                    }
                    continue;
                }
            }
            if text.contains('=') {
                self.assignments.push(text.to_owned());
            } else if source == MacroSource::Makeflags {
                return Err(format!("{location}: invalid MAKEFLAGS operand '{text}'"));
            } else {
                reader::check_target(text, &location).map_err(|error| error.to_string())?;
                self.targets.push(text.to_owned());
            }
            if source == MacroSource::CommandLine {
                parse_options = false;
            }
        }
        Ok(())
    }

    fn makeflags(&self, inherited: &[String]) -> String {
        let mut words: Vec<_> = [
            (self.environment_overrides, "-e"),
            (self.execution.ignore, "-i"),
            (self.execution.keep_going, "-k"),
            (self.execution.dry_run, "-n"),
            (self.execution.silent, "-s"),
            (self.stop_on_error, "-S"),
        ]
        .into_iter()
        .filter(|&(enabled, _)| enabled)
        .map(|(_, option)| option.to_owned())
        .collect();
        for assignment in inherited.iter().chain(&self.assignments) {
            if !assignment.starts_with("MAKEFLAGS=") {
                let mut word = String::new();
                for ch in assignment.chars() {
                    if matches!(ch, '\'' | '"' | '\\' | ' ' | '\t') {
                        word.push('\\');
                    }
                    word.push(ch);
                }
                words.push(word);
            }
        }
        words.join(" ")
    }

    fn select_makefile(&mut self) -> Result<(), String> {
        if !self.makefiles.is_empty() {
            return Ok(());
        }
        for path in ["makefile", "Makefile"] {
            match std::fs::metadata(path) {
                Ok(_) => {
                    self.makefiles.push(path.into());
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(format!("{path}: {error}")),
            }
        }
        // The earlier bmake units accept a single makefile operand.
        if self.targets.len() == 1 && Path::new(&self.targets[0]).is_file() {
            self.makefiles.push(self.targets.remove(0).into());
            return Ok(());
        }
        Err("no makefile found (tried makefile and Makefile)".into())
    }
}

// POSIX.1-2024 make, "Environment Variables": tokenize without carrying out
// shell expansions. Unadorned option letters are the other accepted form.
fn makeflags_arguments(value: &str) -> Result<Vec<OsString>, String> {
    let letters = value.trim_matches([' ', '\t']);
    if !letters.is_empty() && letters.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Ok(vec![format!("-{letters}").into()]);
    }
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, ' ' | '\t') => {
                if !word.is_empty() {
                    args.push(std::mem::take(&mut word).into());
                }
            }
            (None, '\'' | '"') => quote = Some(ch),
            (Some(open), close) if open == close => quote = None,
            (None | Some('"'), '\\') => {
                let next = chars.next().ok_or("trailing backslash in MAKEFLAGS")?;
                if quote.is_some() && !matches!(next, '$' | '`' | '"' | '\\' | '\n') {
                    word.push('\\');
                }
                if next != '\n' {
                    word.push(next);
                }
            }
            _ => word.push(ch),
        }
    }
    if quote.is_some() {
        return Err("unterminated quote in MAKEFLAGS".into());
    }
    if !word.is_empty() {
        args.push(word.into());
    }
    Ok(args)
}

fn make_command() -> Result<String, String> {
    let invoked = env::args_os().next().unwrap_or_else(|| "bmake".into());
    let invoked = invoked
        .into_string()
        .map_err(|_| "invalid UTF-8 in argv[0]")?;
    let path = Path::new(&invoked);
    let command = if path.is_relative() && invoked.contains('/') {
        env::current_dir()
            .map_err(|error| format!("current directory: {error}"))?
            .join(path)
            .into_os_string()
            .into_string()
            .map_err(|_| "invalid UTF-8 in bmake path")?
    } else {
        invoked
    };
    if command
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
    {
        Ok(command)
    } else {
        Ok(format!("'{}'", command.replace('\'', "'\\''")))
    }
}

/// Parse bmake's command-line arguments, change to the requested directory,
/// read the makefiles and build the selected targets.
pub fn execute(arguments: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let make = make_command()?;
    let mut options = Options::default();
    let source = MacroSource::Makeflags;
    if let Some(value) = env::var_os("MAKEFLAGS") {
        let location = source.location();
        let value = value
            .to_str()
            .ok_or_else(|| format!("{location}: invalid UTF-8 in MAKEFLAGS"))?;
        let arguments =
            makeflags_arguments(value).map_err(|error| format!("{location}: {error}"))?;
        options.parse(arguments, source)?;
    }
    let inherited = std::mem::take(&mut options.assignments);
    options.parse(arguments, MacroSource::CommandLine)?;
    options.select_makefile()?;
    let directory = env::current_dir().map_err(|error| format!("current directory: {error}"))?;
    let directory = directory.to_string_lossy().into_owned();
    let environment = env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)));
    let mut initial = Macros::new(options.environment_overrides, directory, environment);
    initial.set_make_defaults(make, options.makeflags(&inherited));
    for (assignments, source) in [
        (&inherited, MacroSource::Makeflags),
        (&options.assignments, MacroSource::CommandLine),
    ] {
        for assignment in assignments {
            initial
                .assign_operand(assignment, source)
                .map_err(|error| error.to_string())?;
        }
    }

    // Delayed remaking can expose new include directives. Bound chains that
    // keep rewriting their own inputs without reaching a stable makefile.
    let mut restarts_left = 64u8;
    let mut streams = HashMap::new();
    loop {
        let mut macros = initial.clone();
        let mut makefile = reader::reload(&options.makefiles, &mut macros, &mut streams)
            .map_err(|error| error.to_string())?;
        let specials = Specials::take(&mut makefile).map_err(|error| error.to_string())?;
        let suffixes = Suffixes::resolve(&mut makefile, &options.targets, &specials)
            .map_err(|error| error.to_string())?;
        if let Some(include) =
            run::remake_includes(&makefile, &macros, &suffixes, &specials, options.execution)?
        {
            restarts_left = restarts_left.checked_sub(1).ok_or_else(|| {
                Error::new(
                    &include.location,
                    format!(
                        "include '{}' keeps changing while remaking makefiles",
                        include.name
                    ),
                )
                .to_string()
            })?;
            continue;
        }
        let graph =
            Graph::for_targets(&makefile, &options.targets).map_err(|error| error.to_string())?;
        return run::build(&graph, &macros, &suffixes, &specials, options.execution);
    }
}
