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

    let tag =
        "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED[pivot_cpu.index()].store(true, Ordering::Relaxed)";
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

#[test]
fn self_retraction_tags_the_fresh_identity_the_resulting_trampoline_reads() {
    // Both wrong-way scenarios this test exists to kill, both a regression to
    // the fixed #735 bug shape:
    //
    //   (a) a designed self-retraction must classify ATTRIBUTED: the tag site
    //       must index by the FRESH identity (`pivot_cpu.index()`), the exact
    //       identity `inline_schedule_trampoline`'s own `CpuId::current()`
    //       read resolves to, since `aarch64_inline_schedule_switch` is a
    //       same-physical-core stack pivot and branch and interrupts stay
    //       masked from the retraction through trampoline entry. If this
    //       regresses to the carried `cpu_id`, the retraction's own resulting
    //       trampoline reads a slot nobody tagged and misclassifies its own
    //       designed recovery as UNEXPLAINED.
    //   (b) the same identity must be the one the classification site reads,
    //       so store and read genuinely agree at these bytes and not merely
    //       by the same name appearing twice with different bindings.
    let source = context_switch_source();

    let old_buggy_tag = "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED[cpu_id].store(true";
    assert!(
        !source.contains(old_buggy_tag),
        "the self-retraction tag site indexes by the carried `cpu_id` again -- this is the \
         exact #735 regression: the retraction's own resulting trampoline runs on the FRESH \
         `pivot_cpu`, not the carried identity, and would misclassify a designed \
         self-retraction as ALREADY_CONSUMED_UNEXPLAINED instead of ATTRIBUTED"
    );

    let fixed_tag =
        "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED[pivot_cpu.index()].store(true, Ordering::Relaxed)";
    assert!(
        source.contains(fixed_tag),
        "expected the self-retraction tag site to index by the fresh `pivot_cpu.index()`; \
         found neither the fixed form nor an unexpected variant -- `{fixed_tag}` is missing"
    );

    // The tag site must sit inside the `pivot_cpu.index() != cpu_id` retraction
    // arm, not merely exist somewhere in the file with the right index
    // variable by coincidence.
    let guard = "if pivot_cpu.index() != cpu_id {";
    let guard_offset = source
        .find(guard)
        .unwrap_or_else(|| panic!("the identity-mismatch retraction guard `{guard}` is missing"));
    let tag_offset = source
        .find(fixed_tag)
        .expect("checked present above");
    let guard_close = source[guard_offset..]
        .find("\n    }\n")
        .map(|relative| guard_offset + relative)
        .unwrap_or(source.len());
    assert!(
        tag_offset > guard_offset && tag_offset < guard_close,
        "the fixed tag site (byte {tag_offset}) is not inside the retraction guard opened at \
         byte {guard_offset} and closed by byte {guard_close}"
    );

    // The trampoline's classification read must use its own fresh `cpu_id`
    // (bound from `CpuId::current()` at trampoline entry) -- the same value
    // the fixed tag site above computed as `pivot_cpu.index()` for the very
    // invocation this retraction produces.
    let read = "COREPROOF_INLINE_HANDOFF_SELF_RETRACTED[cpu_id].swap(false, Ordering::Relaxed)";
    assert!(
        source.contains(read),
        "the trampoline's classification read no longer indexes by its own fresh `cpu_id`; \
         expected `{read}`"
    );
}

#[test]
fn self_retraction_latch_cannot_outlive_one_measured_window() {
    // (b) a stale-latch cross-window read must classify UNEXPLAINED: the tag
    // array must be reset at the driver's window-open edge, so a `true` left
    // by activity before the measured window can never be consumed as
    // ATTRIBUTED by an unrelated null read inside it. If the reset function
    // or its call site regresses away, this test reddens.
    let kernel_source = context_switch_source();
    assert!(
        kernel_source.contains("pub(crate) fn coreproof_reset_self_retracted_tags()"),
        "the self-retraction tag reset function no longer exists in context_switch.rs"
    );
    let reset_body_offset = kernel_source
        .find("fn coreproof_reset_self_retracted_tags()")
        .expect("checked present above");
    let reset_body_end = kernel_source[reset_body_offset..]
        .find("\n}\n")
        .map(|relative| reset_body_offset + relative)
        .unwrap_or(kernel_source.len());
    let reset_body = &kernel_source[reset_body_offset..reset_body_end];
    assert!(
        reset_body.contains("COREPROOF_INLINE_HANDOFF_SELF_RETRACTED")
            && reset_body.contains("store(false"),
        "coreproof_reset_self_retracted_tags's body no longer resets \
         COREPROOF_INLINE_HANDOFF_SELF_RETRACTED to false: {reset_body:?}"
    );
    assert_coreproof_cfg_precedes(
        &kernel_source,
        reset_body_offset,
        "coreproof_reset_self_retracted_tags",
        200,
    );

    let driver_source = read("kernel/src/proof/driver_h.rs");
    assert!(
        driver_source.contains("coreproof_reset_self_retracted_tags();"),
        "driver_h.rs's run() no longer calls coreproof_reset_self_retracted_tags() -- the \
         self-retraction latch can now leak a stale `true` across window boundaries into a \
         later, unrelated specimen (the exact #735 sticky-latch shape)"
    );
    let open_offset = driver_source
        .find("super::coverage::open_window();")
        .expect("driver_h.rs's run() no longer calls coverage::open_window()");
    let reset_offset = driver_source
        .find("coreproof_reset_self_retracted_tags();")
        .expect("checked present above");
    assert!(
        reset_offset > open_offset && reset_offset - open_offset < 300,
        "coreproof_reset_self_retracted_tags() is called {} bytes after \
         coverage::open_window() -- it must sit right at the window-open edge, not somewhere \
         later where organic traffic inside the window could already have set a tag before \
         the reset runs",
        reset_offset.saturating_sub(open_offset)
    );
}
