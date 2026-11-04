use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn source_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

fn markers(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(marker) = map.get("marker").and_then(Value::as_str) {
                if marker.starts_with("[bwm]") {
                    found.push(marker.to_owned());
                }
            }
            for value in map.values() {
                markers(value, found);
            }
        }
        Value::Array(items) => {
            for value in items {
                markers(value, found);
            }
        }
        _ => {}
    }
}

#[test]
fn boot_markers_are_preserved_in_bwm_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog: Value = serde_json::from_slice(&fs::read(root.join("docs/boot-path.json")).unwrap()).unwrap();
    let mut expected = Vec::new();
    markers(&catalog, &mut expected);
    assert!(!expected.is_empty(), "no bwm markers found in boot catalog");
    let mut files = vec![root.join("userspace/programs/src/bwm.rs")];
    source_files(&root.join("userspace/programs/src/bwm"), &mut files);
    let sources: Vec<String> = files.iter().map(|file| fs::read_to_string(file).unwrap()).collect();
    for marker in expected {
        assert!(sources.iter().any(|source| source.contains(&marker)), "missing bwm marker: {marker}");
    }
}

#[test]
fn bwm_has_no_architecture_branches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("userspace/programs/src/bwm.rs")];
    source_files(&root.join("userspace/programs/src/bwm"), &mut files);
    for file in files {
        assert!(!fs::read_to_string(&file).unwrap().contains("target_arch"), "architecture branch in {}", file.display());
    }
}
