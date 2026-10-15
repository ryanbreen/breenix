use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use crate::CmdResult;

fn e(what: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |err| format!("{what}: {err}")
}

pub fn read_write() -> CmdResult {
    fs::write("data.txt", "rust fs\n").map_err(e("write data.txt"))?;
    let mut file = OpenOptions::new().append(true).open("data.txt").map_err(e("open for append"))?;
    file.write_all(b"more\n").map_err(e("append"))?;
    drop(file);
    let text = fs::read_to_string("data.txt").map_err(e("read data.txt"))?;
    println!("contents={}", text.replace('\n', "|"));
    println!("len={}", text.len());
    fs::remove_file("data.txt").map_err(e("remove data.txt"))
}

pub fn dirs() -> CmdResult {
    fs::create_dir_all("a/b/c").map_err(e("create_dir_all a/b/c"))?;
    fs::write("a/x", "x").map_err(e("write a/x"))?;
    fs::write("a/y", "y").map_err(e("write a/y"))?;
    let mut names: Vec<String> = fs::read_dir("a")
        .map_err(e("read_dir a"))?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(e("read a directory entry"))?;
    names.sort();
    println!("a: {}", names.join(","));
    fs::rename("a/x", "a/z").map_err(e("rename a/x a/z"))?;
    println!("renamed={}", fs::metadata("a/z").is_ok() && fs::metadata("a/x").is_err());
    for file in ["a/y", "a/z"] {
        fs::remove_file(file).map_err(e("remove_file"))?;
    }
    for dir in ["a/b/c", "a/b", "a"] {
        fs::remove_dir(dir).map_err(e("remove_dir"))?;
    }
    println!("exists={}", fs::exists("a").map_err(e("exists a"))?);
    Ok(())
}

pub fn metadata() -> CmdResult {
    fs::write("m.txt", "12345").map_err(e("write m.txt"))?;
    fs::set_permissions("m.txt", fs::Permissions::from_mode(0o640)).map_err(e("set_permissions"))?;
    let meta = fs::metadata("m.txt").map_err(e("metadata m.txt"))?;
    println!("len={} file={} dir={} mode={:o}", meta.len(), meta.is_file(), meta.is_dir(), meta.permissions().mode() & 0o777);
    let dir = fs::metadata(".").map_err(e("metadata ."))?;
    println!("dot is_dir={}", dir.is_dir());
    fs::remove_file("m.txt").map_err(e("remove m.txt"))
}
