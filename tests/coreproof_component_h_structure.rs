//! Structural ratchets for core-proof Component H's handoff-slot instrument.
//!
//! These tests parse the real AArch64 context-switch source rather than pinning
//! line numbers. They prove the slot stays consume-on-read, the observation-only
//! instrument compiles solely with `coreproof`, and its three classification
//! counters remain one co-located exhaustive dispatch.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn context_switch_source() -> String {
    read("kernel/src/arch_impl/aarch64/context_switch.rs")
}

#[test]
fn inline_schedule_state_is_only_ever_read_via_a_consumed_swap() {
    let source = context_switch_source();
    for field in ["scheduler_ptr", "should_requeue_old"] {
        let marker = format!(".{field}");
        let offsets: Vec<_> = source
            .match_indices(&marker)
            .map(|(offset, _)| offset)
            .collect();
        assert!(
            !offsets.is_empty(),
            "no `.{field}` access exists; the census parser has drifted off the source"
        );

        for offset in offsets {
            let following = source[offset + marker.len()..].trim_start();
            if following.starts_with(".store(") {
                continue;
            }
            assert!(
                following.starts_with(".swap("),
                "`.{field}` access at byte {offset} is neither a write through `.store(...)` \
                 nor a consume-on-read through `.swap(...)`"
            );

            let statement_start = source[..offset]
                .rfind(|character| matches!(character, ';' | '{' | '}'))
                .map(|position| position + 1)
                .unwrap_or(0);
            let statement_head = source[statement_start..offset].trim_start();
            assert!(
                statement_head.starts_with("let "),
                "`.{field}.swap(...)` at byte {offset} discards its consumed value instead \
                 of binding it in a named `let` statement; statement head: {statement_head:?}"
            );
            assert!(
                !statement_head.starts_with("let _"),
                "`.{field}.swap(...)` at byte {offset} uses a discarded `let _` binding"
            );
        }
    }
}

fn declaration_offset(source: &str, name: &str) -> usize {
    [
        format!("pub(crate) static {name}"),
        format!("static {name}"),
    ]
    .iter()
    .find_map(|marker| source.find(marker))
    .unwrap_or_else(|| panic!("no static declaration found for {name}"))
}

fn assert_coreproof_cfg_precedes(source: &str, offset: usize, description: &str, span: usize) {
    const CFG: &str = "#[cfg(feature = \"coreproof\")]";
    let cfg_offset = source[..offset]
        .rfind(CFG)
        .unwrap_or_else(|| panic!("{description} has no preceding `{CFG}`"));
    assert!(
        offset - cfg_offset < span,
        "{description}'s nearest preceding `{CFG}` is {} bytes away, outside the {span}-byte \
         attachment window",
        offset - cfg_offset
    );
}

#[test]
fn the_new_inline_slot_instrument_is_entirely_coreproof_gated() {
    let source = context_switch_source();
    const CFG: &str = "#[cfg(feature = \"coreproof\")]";
    for name in [
        "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED",
        "COREPROOF_INLINE_SLOT_ENTERED_UNCONSUMED",
        "COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_ATTRIBUTED",
        "COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_UNEXPLAINED",
    ] {
        let declaration = declaration_offset(&source, name);
        let cfg_offset = source[..declaration]
            .rfind(CFG)
            .unwrap_or_else(|| panic!("{name}'s declaration has no preceding `{CFG}`"));
        assert!(
            !source[cfg_offset + CFG.len()..declaration].contains(';'),
            "{name}'s nearest preceding `{CFG}` is separated from the declaration by a \
             terminated statement, so the cfg does not guard this static"
        );
    }

    let tag = "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED[cpu_id].store(true, Ordering::Relaxed)";
    let tag_offset = source
        .find(tag)
        .unwrap_or_else(|| panic!("the self-retraction tag site no longer contains `{tag}`"));
    assert_coreproof_cfg_precedes(&source, tag_offset, "the self-retraction tag site", 300);

    let classification =
        "COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_ATTRIBUTED.fetch_add(1, Ordering::Relaxed)";
    let classification_offset = source.find(classification).unwrap_or_else(|| {
        panic!("the inline-slot classification block no longer contains `{classification}`")
    });
    assert_coreproof_cfg_precedes(
        &source,
        classification_offset,
        "the inline-slot classification block",
        500,
    );
}

#[test]
fn inline_slot_classification_counters_fire_from_exactly_one_site_each_and_stay_co_located() {
    let source = context_switch_source();
    let patterns = [
        "COREPROOF_INLINE_SLOT_ENTERED_UNCONSUMED.fetch_add(1, Ordering::Relaxed)",
        "COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_ATTRIBUTED.fetch_add(1, Ordering::Relaxed)",
        "COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_UNEXPLAINED.fetch_add(1, Ordering::Relaxed)",
    ];
    let mut offsets = Vec::new();
    for pattern in patterns {
        let occurrences: Vec<_> = source
            .match_indices(pattern)
            .map(|(offset, _)| offset)
            .collect();
        assert_eq!(
            occurrences.len(),
            1,
            "classification counter `{pattern}` appears {} times, not exactly once: \
             {occurrences:?}",
            occurrences.len()
        );
        offsets.push(occurrences[0]);
    }

    let min = *offsets.iter().min().expect("three classification offsets");
    let max = *offsets.iter().max().expect("three classification offsets");
    let span = max - min;
    assert!(
        span < 800,
        "the three classification increments span {span} bytes (offsets={offsets:?}), so \
         they are no longer one small co-located if/else dispatch"
    );
}
