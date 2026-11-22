use crate::{graph::Graph, inference::Suffixes, r#macro::Macros, reader, run};
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Options {
    environment_overrides: bool,
    makefiles: Vec<PathBuf>,
    assignments: Vec<String>,
    targets: Vec<String>,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut options = Self::default();
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
                            'e' => options.environment_overrides = true,
                            'f' | 'C' => {
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
                                    options.makefiles.push(path.into());
                                }
                                break;
                            }
                            _ => return Err(format!("unknown option '-{flag}'")),
                        }
                    }
                    continue;
                }
            }
            parse_options = false;
            if text.contains('=') {
                options.assignments.push(text.to_owned());
            } else {
                options.targets.push(text.to_owned());
            }
        }
        options.select_makefile()?;
        Ok(options)
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

/// Parse bmake's command-line arguments, change to the requested directory,
/// read the makefiles and build the selected targets.
pub fn execute(arguments: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let options = Options::parse(arguments)?;
    let directory = env::current_dir().map_err(|error| format!("current directory: {error}"))?;
    let directory = directory.to_string_lossy().into_owned();
    let environment = env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)));
    let mut macros = Macros::new(options.environment_overrides, directory, environment);
    for assignment in &options.assignments {
        macros
            .assign_command_line(assignment)
            .map_err(|error| error.to_string())?;
    }

    let mut makefile =
        reader::load(&options.makefiles, &mut macros).map_err(|error| error.to_string())?;
    let suffixes = Suffixes::resolve(&mut makefile, &options.targets);
    let graph =
        Graph::for_targets(&makefile, &options.targets).map_err(|error| error.to_string())?;
    run::build(&graph, &macros, &suffixes)
}
