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
        "fn held_runs(",
        // Population is production code; MappingIo is the host BreenFS transfer.
        "struct Claim",
        "pub(crate) enum MapError",
        "impl MapError",
        "pub(crate) struct PageClaim",
        "impl Drop for PageClaim",
        "pub(crate) struct Population",
        "impl core::ops::Deref for Population",
        "pub(crate) fn populate(",
        "pub(crate) fn populate_through(",
        // The fill interface (E3-4): batches of claimed frames, their
        // outcomes, the default fill through a transfer, and the host window.
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub enum FillPage",
        "pub struct FrameVec",
        "struct FrameSegment",
        "impl FrameVec",
        "pub struct FillBatch",
        "impl FillBatch",
        "pub fn fill_by_reads(",
        "pub mod fill_window",
        "pub(crate) fn fill_counts(",
        // Exec admission and the running-image write rule (E3-3) are production.
        "impl Drop for WriteOpen",
        "impl Drop for ExecLoad",
        "pub(crate) struct ExecImage",
        "impl<'a> ExecImage<'a>",
        "pub(crate) fn exec_copy_fallbacks(",
        "pub(crate) fn note_exec_fallback(",
        "pub(crate) fn etxtbsy_refusals(",
        // Exec's cached PT_LOAD: its exec binding and the entries it installs
        // (E3-3); the page table, VMA and `bind` record are host stand-ins.
        "impl ExecImage<'_>",
    ] {
        out.push_str(&item(&source, marker));
        out.push('\n');
    }
    // The entry flags exec and the fault path install, over host flag bits.
    out.push_str(&item(&source, "fn entry_flags(")
        .replace("crate::syscall::memory_common::prot_to_page_flags", "prot_to_page_flags"));
    out.push('\n');
    let common = root.join("kernel/src/syscall/memory_common.rs");
    println!("cargo:rerun-if-changed={}", common.display());
    out.push_str(&item(&fs::read_to_string(common).unwrap(), "pub fn prot_to_page_flags("));
    out.push('\n');
    // Binding lifetime is production code; only the host owner's type/name differ.
    out.push_str(&item(&source, "impl Drop for Binding")
        .replace("impl Drop for Binding", "impl Drop for HostBindingOwner")
        .replace("self.handle", "self.node"));
    out.push('\n');
    // The refusal error that bind and mprotect report as EBUSY (DEV-44).
    let marker = "pub(crate) const ROOT_FROZEN";
    let start = source.find(marker).unwrap_or_else(|| panic!("missing {marker}"));
    let end = start + source[start..].find(";\n").unwrap();
    out.push_str(&source[start..=end]);
    out.push('\n');
    // The fault's page choice and its absent-page count (E3-2) are production.
    // The running-image refusal (E3-3) mmap reports as ETXTBSY is production too.
    for marker in ["static ABSENT_BOUND", "static EXEC_COPY_FALLBACKS", "static ETXTBSY_REFUSALS",
        "static FILLS", "static FILL_PAGES", "static FILL_ERRORS", "static FILL_DISCARDED",
        "pub(crate) struct WriteOpen(", "struct ExecLoad(", "pub(crate) const TEXT_BUSY"] {
        let start = source.find(marker).unwrap_or_else(|| panic!("missing {marker}"));
        let end = start + source[start..].find(";\n").unwrap();
        out.push_str(&source[start..=end]);
        out.push('\n');
    }
    out.push_str(&item(&source, "pub(crate) fn absent_bound("));
    out.push('\n');
    out.push_str("impl MapInner {\n");
    for marker in ["    fn bound(", "    fn deny_write(", "    fn writers(", "    fn refuse_text_busy(", "    fn make_executable(", "    fn gain_writable_shared(", "    fn mark_populating(", "    fn unbacked(", "    fn new_holes(", "    fn reserve_holes(", "    fn add_holes(", "    fn drop_holes(", "    fn reserve_poison(", "    fn poison(", "    fn fault_frame("] {
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
        "    pub(crate) fn writable_shared(",
        "    pub(crate) fn open_writer(",
        "    pub(crate) fn refuse_text_busy(",
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
