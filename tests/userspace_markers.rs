//! A kernel announcement must never satisfy a userspace boot stage.

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
    assert!(!markers.is_empty(), "No userspace markers found");

    let mut violations = Vec::new();
    for path in files(&root.join("kernel/src")) {
        let source = fs::read_to_string(&path).unwrap();
        for (line, text) in source.lines().enumerate() {
            for marker in &markers {
                if text.contains(marker) {
                    violations.push(format!(
                        "{}:{}: {marker}",
                        path.strip_prefix(root).unwrap().display(),
                        line + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Userspace markers in kernel source:\n{}",
        violations.join("\n")
    );
}
