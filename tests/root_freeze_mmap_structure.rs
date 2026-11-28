//! Paired generations DEV-44(2a), review of PR #3273: a shared-write mmap of
//! a booted-root file that the root freeze refuses must leave the caller's
//! address space as it was.
//!
//! `map_file` in kernel/src/syscall/mmap.rs clears the target range with
//! `clear_for_mapping`, which unmaps whatever a MAP_FIXED request replaces,
//! and then registers the binding. If the freeze refusal is decided only by
//! `file_map::bind`, after the clear, a refused MAP_FIXED has already removed
//! the old mapping and its private pages. So the shared write authority must
//! be claimed (counted, then checked against the freeze, under the file's map
//! lock) before the clear, and the claim handed to `bind`.
//!
//! mmap.rs needs the process manager and page tables, so the host cannot run
//! it; this reads the production source.
//!
//! Run: scripts/run-structure-tests.sh root_freeze_mmap_structure

use std::fs;
use std::path::PathBuf;

fn source(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The text of `fn name` up to the next top-level `fn` or the file's end.
fn function<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &text[start..];
    let end = rest[1..]
        .find("\nfn ")
        .or_else(|| rest[1..].find("\npub fn "))
        .or_else(|| rest[1..].find("\npub(crate) fn "))
        .map_or(rest.len(), |at| at + 1);
    &rest[..end]
}

fn position(body: &str, needle: &str, what: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("map_file has no {what} (`{needle}`)"))
}

#[test]
fn map_file_claims_shared_write_before_clearing_the_range() {
    let text = source("kernel/src/syscall/mmap.rs");
    let body = function(&text, "map_file");
    let claim = position(body, "file_map::claim_shared_write(", "shared write claim");
    let clear = position(body, "clear_for_mapping(", "range clear");
    let bind = position(body, "file_map::bind(", "binding");
    assert!(
        claim < clear,
        "map_file clears the range before claiming shared write authority: a refused \
         MAP_FIXED would lose the mapping it replaces"
    );
    assert!(clear < bind, "map_file binds before clearing the range");
    let call = &body[bind..];
    let args = &call[..call.find(") {").expect("the bind call's argument list")];
    assert!(args.contains("claim"), "map_file does not hand its claim to file_map::bind: {args}");
}

#[test]
fn bind_takes_the_claim_rather_than_deciding_the_refusal() {
    let text = source("kernel/src/memory/file_map.rs");
    let body = function(&text, "bind");
    assert!(
        body.contains("SharedWriteClaim"),
        "file_map::bind must take the caller's SharedWriteClaim"
    );
    assert!(
        !body.contains("gain_writable_shared("),
        "file_map::bind decides the freeze refusal itself, after the caller cleared the range"
    );
}
