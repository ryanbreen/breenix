#![forbid(unsafe_code)]

pub mod remote;
mod sftp;
pub mod tree;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn normal_relative(path: &str) -> bool {
    path.split('/').all(|part| {
        !part.is_empty() && part != "." && part != ".." && !part.chars().any(char::is_control)
    })
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}
