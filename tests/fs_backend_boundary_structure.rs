use std::fs;
use std::path::{Path, PathBuf};

/// The staged migration in docs/fs/cowfs-design.md section 1.1 lowers this
/// ext2-only ceiling at each stage and never raises it.
/// claim-lint:ok: Staged migration requirement in docs/fs/cowfs-design.md section 1.1.
const CEILING: usize = 184;

fn visit(directory: &Path, ext2_dir: &Path, tally: &mut Vec<(PathBuf, usize)>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()))
    {
        let path = entry.expect("read directory entry").path();
        if path == ext2_dir {
            continue;
        }
        if path.is_dir() {
            visit(&path, ext2_dir, tally);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let count = source
                .lines()
                .filter(|line| line.contains("fs::ext2") || line.contains("ext2::"))
                .count();
            if count > 0 {
                tally.push((path, count));
            }
        }
    }
}

#[test]
fn ext2_references_outside_ext2_stay_under_ceiling() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let kernel = root.join("kernel/src");
    let mut tally = Vec::new();
    visit(&kernel, &kernel.join("fs/ext2"), &mut tally);
    tally.sort();
    let total: usize = tally.iter().map(|(_, count)| count).sum();
    let details = tally
        .iter()
        .map(|(path, count)| format!("{}: {count}", path.strip_prefix(root).unwrap().display()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        total <= CEILING,
        "ext2 references outside fs/ext2: {total} lines (ceiling {CEILING})\n{details}"
    );
}
