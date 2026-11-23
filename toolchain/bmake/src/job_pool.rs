//! bmake's filesystem token pool for POSIX.1-2024 make's -j option.

use std::{
    env,
    fs::{self, DirBuilder},
    io::{self, ErrorKind},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process,
};

pub(crate) const FLAG_PREFIX: &str = "BMAKE_JOB_POOL=";

pub(crate) struct JobPool {
    directory: PathBuf,
    jobs: usize,
    owner: bool,
}

pub(crate) struct Token(PathBuf);

impl JobPool {
    pub(crate) fn open(jobs: usize, inherited: Option<&str>) -> io::Result<Self> {
        if let Some((count, directory)) = inherited.and_then(|value| value.split_once(':')) {
            if count.parse::<usize>() == Ok(jobs) {
                let directory = PathBuf::from(directory);
                if !directory.is_absolute() || !directory.metadata()?.is_dir() {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "invalid token directory",
                    ));
                }
                return Ok(Self {
                    directory,
                    jobs,
                    owner: false,
                });
            }
        }

        // Use an absolute name so recipes can change directory before invoking make.
        let temporary = env::temp_dir().canonicalize()?;
        if temporary.to_str().is_none() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "non-UTF-8 temporary directory",
            ));
        }
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        for sequence in 0.. {
            let directory = temporary.join(format!("bmake-{}-{sequence}", process::id()));
            match builder.create(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        directory,
                        jobs,
                        owner: true,
                    })
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }

    pub(crate) fn makeflags_word(&self) -> String {
        format!("{FLAG_PREFIX}{}:{}", self.jobs, self.directory.display())
    }

    // Each make has one implicit slot. Its calling recipe is already
    // occupying that slot for the entire synchronous sub-make invocation.
    pub(crate) fn try_acquire(&self) -> io::Result<Option<Token>> {
        for index in 1..self.jobs {
            let path = self.directory.join(index.to_string());
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Some(Token(path))),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        if self.owner {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
