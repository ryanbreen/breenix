//! Reject literal userspace catalog markers in kernel source.
//! Source matching cannot detect arbitrary runtime formatting. Set
//! BREENIX_MARKER_SERIAL to serial paths (platform path separator) to check
//! formatted kernel log lines from boots as well.

use proc_macro2::{TokenStream, TokenTree};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn rust_markers(tokens: TokenStream, markers: &mut BTreeSet<String>) {
    let tokens: Vec<_> = tokens.into_iter().collect();
    for window in tokens.windows(3) {
        if matches!(&window[0], TokenTree::Ident(name) if name == "marker")
            && matches!(&window[1], TokenTree::Punct(punct) if punct.as_char() == ':')
        {
            if let TokenTree::Literal(literal) = &window[2] {
                let text: syn::LitStr = syn::parse_str(&literal.to_string()).unwrap();
                markers.extend(text.value().split('|').map(str::to_owned));
            }
        }
    }
    for token in tokens {
        if let TokenTree::Group(group) = token {
            rust_markers(group.stream(), markers);
        }
    }
}

fn json_markers(value: &serde_json::Value, markers: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(marker) = object.get("marker").and_then(|value| value.as_str()) {
                markers.extend(marker.split('|').filter(|part| !part.is_empty()).map(str::to_owned));
            }
            for child in object.values() {
                json_markers(child, markers);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                json_markers(child, markers);
            }
        }
        _ => {}
    }
}

fn marker_matches(text: &str, marker: &str) -> bool {
    // The boot-path catalog uses record suffixes for parameterized programs
    // and suites. Require the emitting program's record prefix as well.
    match marker {
        " START" | " DONE PASS exit 0; no FAIL output" => {
            text.contains("RUN ") && text.contains(marker)
        }
        " START cases=" | " DONE passed=" => {
            text.contains("SUITE ") && text.contains(marker)
        }
        _ => text.contains(marker),
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // Build outputs are not program source.
            if path.file_name().unwrap() != "target" {
                result.extend(files(&path));
            }
        } else {
            result.push(path);
        }
    }
    result.sort();
    result
}

#[test]
fn userspace_stage_markers_do_not_occur_in_kernel_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut markers = BTreeSet::new();
    let catalog = fs::read_to_string(root.join("xtask/src/boot_stages.rs")).unwrap();
    rust_markers(catalog.parse().unwrap(), &mut markers);
    assert!(!markers.is_empty(), "Rust catalog has no markers");

    let resources = root.join("tools/breenix-runs/Sources/BreenixRuns/Resources");
    let mut catalogs = 0;
    for path in files(&resources) {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.starts_with("boot-stages-") && name.ends_with(".json") {
            let catalog: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            for stage in catalog["stages"].as_array().unwrap() {
                markers.extend(
                    stage["marker"]
                        .as_str()
                        .unwrap()
                        .split('|')
                        .map(str::to_owned),
                );
            }
            catalogs += 1;
        }
    }
    assert!(catalogs > 0, "JSON catalogs missing");

    let sources: Vec<String> = files(&root.join("userspace"))
        .into_iter()
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("rs" | "c" | "h" | "S" | "s" | "asm")
            )
        })
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    markers.retain(|marker| {
        !marker.is_empty() && sources.iter().any(|source| source.contains(marker))
    });
    // Explicit markers on userspace milestones belong to PID 1, even when the
    // emitter assembles them dynamically or no current literal emitter exists.
    let boot_path: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("docs/boot-path.json")).unwrap())
            .unwrap();
    for milestone in boot_path["milestones"].as_array().unwrap() {
        if milestone["kernel"].as_bool() != Some(true) {
            json_markers(&milestone["stages"], &mut markers);
        }
    }
    // This catalog stage's emitter uses formatted fragments.
    markers.insert("HTTP_TEST: https_rejected OK".to_owned());
    assert!(!markers.is_empty(), "No userspace markers found");

    let mut violations = Vec::new();
    for path in files(&root.join("kernel/src")) {
        let source = fs::read_to_string(&path).unwrap();
        for (line, text) in source.lines().enumerate() {
            for marker in &markers {
                if marker_matches(text, marker) {
                    violations.push(format!(
                        "{}:{}: {marker}",
                        path.strip_prefix(root).unwrap().display(),
                        line + 1
                    ));
                }
            }
        }
    }
    if let Some(serials) = std::env::var_os("BREENIX_MARKER_SERIAL") {
        for path in std::env::split_paths(&serials) {
            let serial = fs::read_to_string(&path).unwrap();
            for (line, text) in serial.lines().enumerate() {
                // Both logger implementations identify kernel module targets;
                // buffered early log lines carry [BUFF] instead of a level.
                if text.contains("kernel::") || text.contains("] kernel:") {
                    for marker in &markers {
                        if marker_matches(text, marker) {
                            violations.push(format!("{}:{}: {marker}", path.display(), line + 1));
                        }
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Userspace markers in kernel source or serial logs:\n{}",
        violations.join("\n")
    );
}
