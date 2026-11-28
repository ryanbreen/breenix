//! A shared mmap with write authority of a running image is refused with
//! ETXTBSY (docs/plans/e3-page-cache.md 10.3). The refusal must come before
//! `map_file` clears the destination, so a refused MAP_FIXED request leaves
//! the mapping it would have replaced; and the request keeps its descriptor's
//! open description, whose write access counts in the file's writers, until
//! the binding is published, so a close meanwhile cannot let an exec start
//! sharing the file's pages.
//!
//! The binding gate makes the early check final: mmap holds it shared from
//! before the check through `bind`, and `deny_write` rises from 0 only while
//! an exec holds it exclusively (`fs/vfs/exec.rs`, `with_image`).
const MMAP: &str = include_str!("../../../kernel/src/syscall/mmap.rs");

/// The item that starts at `start`, through its closing brace at column 0.
fn item<'a>(source: &'a str, start: &str) -> &'a str {
    let from = source.find(start).unwrap_or_else(|| panic!("missing {start}"));
    let rest = &source[from..];
    &rest[..rest.find("\n}\n").unwrap_or_else(|| panic!("unclosed {start}")) + 2]
}

/// `source` without its line comments.
fn code(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_running_image_is_refused_before_the_destination_is_cleared() {
    let map_file = code(item(MMAP, "fn map_file("));
    let clear = map_file.find("clear_for_mapping(").expect("map_file clears its destination");
    let refusal = map_file.find(".refuse_text_busy(").unwrap_or(usize::MAX);
    assert!(
        refusal < clear,
        "map_file clears the destination before it checks for a running image, so a refused MAP_FIXED loses the old mapping",
    );
}

#[test]
fn the_request_keeps_its_open_description_through_publication() {
    let file_mapping = code(item(MMAP, "fn file_mapping("));
    assert!(
        file_mapping.contains("descriptor.clone()"),
        "file_mapping returns only the VNode and a snapshot of write access, not the open description",
    );
    let map_file = code(item(MMAP, "fn map_file("));
    let parameters = &map_file[..map_file.find('{').unwrap()];
    assert!(
        parameters.contains("FileDescriptor"),
        "map_file does not hold the descriptor's open description while it publishes the binding",
    );
}
