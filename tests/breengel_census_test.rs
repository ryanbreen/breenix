#[path = "../libs/libbreenix/src/suite_stage/census.rs"]
mod census;

use std::fs;
use std::path::PathBuf;

fn fixture(name: &str, statuses: &[Option<(&str, usize)>]) -> PathBuf {
    let root = std::env::current_dir().unwrap().join("target/breengel-census-fixtures").join(name);
    if root.exists() { fs::remove_dir_all(&root).unwrap(); }
    fs::create_dir_all(&root).unwrap();
    for (i, status) in statuses.iter().enumerate() {
        let dir = root.join((i + 1).to_string());
        fs::create_dir(&dir).unwrap();
        if let Some((name, kb)) = status {
            fs::write(dir.join("status"), format!("Name:\t{name}\nVmPTE:\t{kb} kB\n")).unwrap();
        }
    }
    root
}

#[test]
fn complete_censuses_rank_all_available_processes() {
    for (name, statuses, total, top) in [
        ("one", vec![Some(("init", 20))], 20, "1:init:20"),
        ("two", vec![Some(("init", 20)), Some(("runner", 12))], 32, "1:init:20,2:runner:12"),
        ("four", vec![Some(("init", 20)), Some(("runner", 12)), Some(("worker", 8)), Some(("other", 4))], 44, "1:init:20,2:runner:12,3:worker:8"),
    ] {
        let root = fixture(name, &statuses);
        assert_eq!(census::read(&root), Some((statuses.len(), total, top.to_string())));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn missing_or_invalid_status_never_yields_a_partial_census() {
    let root = fixture("failed", &[Some(("init", 20)), None]);
    assert_eq!(census::read(&root), None);
    fs::write(root.join("2/status"), "Name:\tbad\nVmPTE: nope\n").unwrap();
    assert_eq!(census::read(&root), None);
    fs::remove_dir_all(root).unwrap();
    let root = fixture("empty", &[]);
    assert_eq!(census::read(&root), None);
    fs::remove_dir_all(root).unwrap();
}
