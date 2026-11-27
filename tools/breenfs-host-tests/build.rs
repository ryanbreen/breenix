// Compile the mapping entry points against pinned host pages, without a guest
// frame allocator or page tables. The traversal/copy/barrier code is production.
use std::{env, fs, path::PathBuf};

fn item(source: &str, marker: &str) -> String {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing {marker}"));
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source.bytes().enumerate().skip(brace) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=offset].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unclosed {marker}");
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let path = root.join("kernel/src/memory/file_map.rs");
    println!("cargo:rerun-if-changed={}", path.display());
    let source = fs::read_to_string(path).unwrap();
    let mut out = String::new();
    for marker in [
        "fn pages(",
        "#[derive(Clone, Copy, Debug)]\nstruct WritebackReceipt",
        "struct WritebackPage",
        "struct PageVec<T>",
        "impl<T> PageVec<T>",
        "fn bound_runs(",
        // Population is production code; MappingIo is the host BreenFS transfer.
        "pub(crate) enum MapError",
        "pub(crate) fn populate(",
    ] {
        out.push_str(&item(&source, marker));
        out.push('\n');
    }
    // Binding lifetime is production code; only the host owner's type/name differ.
    out.push_str(&item(&source, "impl Drop for Binding")
        .replace("impl Drop for Binding", "impl Drop for HostBindingOwner")
        .replace("self.handle", "self.node"));
    out.push('\n');
    out.push_str("impl MapInner {\n");
    for marker in ["    fn bound(", "    fn holes(", "    fn reserve_poison(", "    fn poison("] {
        out.push_str(&item(&source, marker));
        out.push('\n');
    }
    out.push_str("}\n");
    for marker in [
        "pub(crate) fn writeback_buffer(",
        "pub(crate) fn sync_range(",
        "pub(crate) fn stage_range_with_buffer(",
        "fn write_range_with_buffer(",
        "pub(crate) struct SyncMaps",
        "impl<'a, T: core::ops::Deref<Target = VNode>> SyncMaps",
        "impl<T: core::ops::Deref<Target = VNode>> Drop for SyncMaps",
    ] {
        out.push_str(&item(&source, marker));
        out.push('\n');
    }
    out.push_str("impl MapState {\n");
    for marker in [
        "    fn refresh(",
        "    pub(crate) fn overlay(",
        "    pub(crate) fn resync_unbound(",
        "    pub(crate) fn prepare_write(",
        "    pub(crate) fn commit_write(",
        "    pub(crate) fn prepare_resize(",
        "    pub(crate) fn invalidate(",
        "    pub(crate) fn transition(",
        "    pub(crate) fn repair(",
        "    pub(crate) fn abandon_population(",
        "    pub(crate) fn has_dirty(",
        "    pub(crate) fn nonempty(",
        "    pub(crate) fn request_writeback(",
        "    fn requeue_writeback(",
        "    pub(crate) fn writeback_pending(",
        "    pub(crate) fn writeback(",
        "    fn writeback_snapshot(",
        "    pub(crate) fn complete_writeback(",
        "    pub(crate) fn complete_writeback_through(",
        "    pub(crate) fn service_writeback(",
        "    pub(crate) fn eviction_pending(",
        "    pub(crate) fn discard_unobserved(",
        "    pub(crate) fn evict(",
    ] {
        out.push_str(&item(&source, marker));
        out.push('\n');
    }
    // Transactional service barriers may supply their own generation publisher.
    let marker = "    pub(crate) fn service_writeback_with(";
    if source.contains(marker) {
        out.push_str(&item(&source, marker));
    }
    out.push_str("}\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("mapping_entries.rs"),
        out.replace("pub(crate)", "pub"),
    )
    .unwrap();
}
