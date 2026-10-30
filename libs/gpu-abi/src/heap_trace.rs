//! Fixed, address-free diagnostic snapshot for the native suite's case boundaries.
pub const WORDS: usize = 32;
pub const SIZE: usize = WORDS * 8;
pub const NAMES: [&str; WORDS] = [
    "schema", "heap_free", "heap_high_water", "frame_frontier", "frame_freelist_bytes",
    "frame_ledger_bytes", "cpu_stage_slots", "cpu_stage_metadata_bytes", "cpu_stage_pages",
    "core_contexts", "core_resources", "core_submissions", "core_syncs", "core_backlog",
    "core_backlog_bytes", "core_backing_meta_sum", "core_context_vector_bytes",
    "native_resources", "native_retired", "native_retired_vector_bytes", "native_active_region_bytes",
    "native_vm_table_bytes", "native_queue_vector_bytes", "native_retired_requests_bytes",
    "native_channel_initial_bytes", "native_channel_parking_bytes", "native_bulk_capacity_bytes",
    "console_terminal_bytes", "console_renderer_vector_bytes", "heap_total", "native_contexts",
    "native_alias_indices_bytes",
];
pub fn decode(bytes: &[u8]) -> Option<[u64; WORDS]> {
    if bytes.len() != SIZE { return None; }
    let mut words = [0; WORDS];
    for (out, chunk) in words.iter_mut().zip(bytes.chunks_exact(8)) {
        *out = u64::from_le_bytes(chunk.try_into().ok()?);
    }
    (words[0] == 1).then_some(words)
}
