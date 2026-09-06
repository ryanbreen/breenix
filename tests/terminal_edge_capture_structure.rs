//! Structural ratchet for failure-capture PR-4: the terminal edges emit a
//! `[BXCAP]` record.
//!
//! PR-3 built the emitter and wired one edge to it -- a self-test that fires
//! from a timer tick behind a feature no gate builds. PR-4 wires the edges a
//! failing boot actually takes: the kernel panic handler on both
//! architectures, and section 7 of the aarch64 fatal postmortem. Whether
//! those call sites are still there is not visible in any type, so it is
//! pinned here.
//!
//! # Census-anchored, not a file list
//!
//! The panic-handler census covers the `#[panic_handler]` items under
//! `kernel/src` as they exist on disk. A handler added tomorrow is counted
//! tomorrow, and an empty census is a failure rather than a pass. The counts
//! of live and stub handlers are anchored so that a new handler, or a live
//! one quietly reduced to a stub, has to be looked at rather than absorbed.
//!
//! # What "live" and "stub" mean here
//!
//! This tree carries 5 `#[panic_handler]` items and 2 of them are
//! cross-architecture stubs: `kernel/src/main.rs`'s `aarch64_stub` and
//! `kernel/src/main_aarch64.rs`'s `non_aarch64_stub` exist only so each
//! binary still has a lang item when it is compiled for the architecture it
//! is not written for. Their bodies are `loop {}` and they have no state to
//! capture, since the binary they belong to does not run. The
//! classification is computed from the body, not from the file name, and a
//! live handler edited down to `loop {}` moves between the 2 anchored counts
//! -- which is a red, not a silent pass.
//!
//! # What this suite is worth
//!
//! It reads source text. It can see that the call site is present and where
//! it sits relative to the terminating construct beside it; it cannot see
//! that the capture ran or that its bytes reached the wire. That evidence is
//! the committed serials under
//! `docs/planning/green-program/failure-capture/serials/pr4/`, decoded by
//! `tests/capture_bxcap_schema_structure.rs`.

use std::fs;
use std::path::{Path, PathBuf};

const KERNEL_SRC: &str = "kernel/src";
const GATE_DIR: &str = "docker/qemu";
const EXCEPTION_SOURCE: &str = "kernel/src/arch_impl/aarch64/exception.rs";

/// The emitter call a live terminal edge has to reach.
const EMIT_CALL: &str = "capture::emit(";

/// Live panic handlers in `kernel/src`: x86_64's in `main.rs`, aarch64's in
/// `main_aarch64.rs`, and the library's `#[cfg(test)]` one in `lib.rs`.
const LIVE_PANIC_HANDLERS: usize = 3;

/// Cross-architecture stubs: `main.rs`'s `aarch64_stub` and
/// `main_aarch64.rs`'s `non_aarch64_stub`.
const STUB_PANIC_HANDLERS: usize = 2;

/// The sections `dump_fatal_postmortem_once` claims, 0 through 7.
const FATAL_SECTION_COUNT: usize = 8;

/// The section PR-4 wires the capture into. 0 through 6 are untouched.
const CAPTURE_SECTION: usize = 7;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

// ---------------------------------------------------------------------------
// A code mask, so a brace inside a string or a comment is not a brace
// ---------------------------------------------------------------------------

/// `true` at each byte that is code: not inside a `//` or `/* */` comment,
/// not inside a string, byte-string or character literal.
///
/// The panic handlers this suite reads contain `serial_println!("... {} ...")`,
/// so a naive brace match would end a body inside a format string. A
/// lifetime (`'static`) is not a character literal and is deliberately not
/// treated as one: `'` opens a literal only when it is followed by an escape
/// or by one character and a closing quote.
fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    mask[i] = false;
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                mask[i] = false;
                mask[i + 1] = false;
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        mask[i] = false;
                        mask[i + 1] = false;
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        mask[i] = false;
                        mask[i + 1] = false;
                        i += 2;
                        continue;
                    }
                    mask[i] = false;
                    i += 1;
                }
            }
            b'"' => {
                mask[i] = false;
                i += 1;
                while i < bytes.len() {
                    mask[i] = false;
                    if bytes[i] == b'\\' {
                        if i + 1 < bytes.len() {
                            mask[i + 1] = false;
                        }
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                let escaped = bytes.get(i + 1) == Some(&b'\\');
                let one_char = bytes.get(i + 2) == Some(&b'\'');
                if escaped || one_char {
                    mask[i] = false;
                    i += 1;
                    while i < bytes.len() {
                        mask[i] = false;
                        if bytes[i] == b'\\' {
                            if i + 1 < bytes.len() {
                                mask[i + 1] = false;
                            }
                            i += 2;
                            continue;
                        }
                        if bytes[i] == b'\'' {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    mask
}

/// The span `(open, close)` of the brace-delimited block that starts at the
/// first code `{` at or after `from`.
///
/// Anti-vacuity: an unbalanced source yields no span rather than one that
/// runs to end of file, and each caller treats that as a failure.
fn block_span(source: &str, mask: &[bool], from: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let open = (from..bytes.len()).find(|i| mask[*i] && bytes[*i] == b'{')?;
    let mut depth = 0usize;
    for i in open..bytes.len() {
        if !mask[i] {
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// The source with COMMENT bytes replaced by spaces, offsets and length
/// preserved. String literals are left alone on purpose: one of the
/// terminating constructs this suite orders against is the `"wfi"` operand of
/// an `asm!`, which lives inside a string.
///
/// The `contains`/`find` calls below run over this view rather than the raw
/// text, so a construct NAMED IN A COMMENT is not mistaken for one that is
/// there. That distinction is not academic: this PR's own comments name
/// `exit_qemu` while explaining why the capture precedes it, and the first
/// revision of this suite read those words as the call.
fn code_only(source: &str) -> String {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // A string literal is masked out by `code_mask` but must survive
        // here, so it is recognised and copied whole.
        if mask[i] || bytes[i] == b'"' || in_string_literal(source, &mask, i) {
            out.push(char::from(bytes[i]));
        } else {
            out.push(' ');
        }
        i += 1;
    }
    out
}

/// Whether byte `i`, which `code_mask` marked as non-code, is inside a
/// double-quoted string rather than a comment. The nearest preceding
/// non-code run start decides it.
fn in_string_literal(source: &str, mask: &[bool], i: usize) -> bool {
    let bytes = source.as_bytes();
    let mut start = i;
    while start > 0 && !mask[start - 1] {
        start -= 1;
    }
    bytes.get(start) == Some(&b'"')
}

/// The source with comments and whitespace removed, for shape comparisons.
fn compact(source: &str) -> String {
    let mask = code_mask(source);
    source
        .as_bytes()
        .iter()
        .zip(&mask)
        .filter_map(|(byte, code)| {
            (*code && !byte.is_ascii_whitespace()).then_some(char::from(*byte))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The panic-handler census
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PanicHandler {
    file: String,
    /// The handler function's body, braces included, as written.
    body: String,
    /// The same body with comment bytes blanked, which is what the
    /// presence and ordering checks below actually read.
    code: String,
}

impl PanicHandler {
    /// A cross-architecture lang-item stub: a body whose one statement is a
    /// diverging loop. There is no state to capture in a binary that is not
    /// the one being run.
    fn is_stub(&self) -> bool {
        compact(&self.body) == "{loop{}}"
    }
}

/// The `.rs` files under `kernel/src`, recursively.
fn kernel_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("kernel/src must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&repo_path(KERNEL_SRC), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable source");
            (name, body)
        })
        .collect()
}

/// The `#[panic_handler]` items under `kernel/src`, each with its body.
fn panic_handlers_in(sources: &[(String, String)]) -> Vec<PanicHandler> {
    let mut handlers = Vec::new();
    for (name, source) in sources {
        let mask = code_mask(source);
        let mut cursor = 0usize;
        while let Some(offset) = source[cursor..].find("#[panic_handler]") {
            let at = cursor + offset;
            cursor = at + 1;
            if !mask[at] {
                // Inside a comment: an attribute quoted in prose is not one.
                continue;
            }
            let (open, close) = block_span(source, &mask, at).unwrap_or_else(|| {
                panic!("{name}: could not find the body of the #[panic_handler] at byte {at}")
            });
            let body = source[open..=close].to_string();
            assert!(
                body.len() < 4096,
                "{name}: the body extracted for the #[panic_handler] at byte {at} is \
                 {} bytes, which means the brace match ran past the function. This \
                 suite's conclusions would be about the wrong text.",
                body.len()
            );
            handlers.push(PanicHandler {
                file: name.clone(),
                code: code_only(&body),
                body,
            });
        }
    }
    handlers
}

fn panic_handlers() -> Vec<PanicHandler> {
    panic_handlers_in(&kernel_sources())
}

/// The assertion body, shared by the real-source test and its anti-vacuity
/// legs: the census splits into anchored counts, and each live handler
/// reaches the emitter.
fn assert_every_live_handler_captures(handlers: &[PanicHandler]) {
    assert!(
        !handlers.is_empty(),
        "no #[panic_handler] found anywhere under {KERNEL_SRC}; this census is \
         checking nothing"
    );

    let (stubs, live): (Vec<_>, Vec<_>) = handlers.iter().partition(|h| h.is_stub());

    assert_eq!(
        stubs.len(),
        STUB_PANIC_HANDLERS,
        "expected {STUB_PANIC_HANDLERS} cross-architecture `loop {{}}` panic-handler \
         stubs under {KERNEL_SRC}, found {} ({:?}). A live handler reduced to a stub \
         lands here instead of in the live count; a new stub has to be argued for.",
        stubs.len(),
        stubs.iter().map(|h| &h.file).collect::<Vec<_>>()
    );

    assert_eq!(
        live.len(),
        LIVE_PANIC_HANDLERS,
        "expected {LIVE_PANIC_HANDLERS} live panic handlers under {KERNEL_SRC}, found \
         {} ({:?}). A handler added here is a terminal edge that has to emit a \
         capture, so the count is anchored deliberately.",
        live.len(),
        live.iter().map(|h| &h.file).collect::<Vec<_>>()
    );

    for handler in &live {
        assert!(
            handler.code.contains(EMIT_CALL),
            "{}: a live #[panic_handler] must reach the capture emitter \
             (`{EMIT_CALL}`). Without it a KERNEL PANIC prints a message and no \
             state, which is the gap failure-capture PR-4 closes:\n{}",
            handler.file,
            handler.body
        );
        assert!(
            handler.code.contains("Edge::Panic"),
            "{}: the capture a panic handler emits must carry Edge::Panic, or the \
             `edge=` field a scorer greps for names the wrong edge",
            handler.file
        );
    }
}

#[test]
fn every_live_panic_handler_in_the_kernel_emits_a_capture() {
    assert_every_live_handler_captures(&panic_handlers());
}

/// Rewrite the body of the first `#[panic_handler]` that reaches the emitter,
/// in memory. Returns the file it mutated so a leg can name it, and panics
/// rather than returning quietly when it finds no such handler: a mutation
/// leg that changed no bytes would pass for the wrong reason.
fn mutate_first_capturing_handler(
    sources: &mut [(String, String)],
    rewrite: impl Fn(&str) -> String,
) -> String {
    for (name, source) in sources.iter_mut() {
        let mask = code_mask(source);
        let Some(at) = (0..source.len())
            .find(|i| mask[*i] && source[*i..].starts_with("#[panic_handler]"))
        else {
            continue;
        };
        let (open, close) = block_span(source, &mask, at).expect("a handler body");
        let body = source[open..=close].to_string();
        if !code_only(&body).contains(EMIT_CALL) {
            continue;
        }
        let rewritten = rewrite(&body);
        assert_ne!(
            rewritten, body,
            "{name}: the mutation left the handler body unchanged, so this leg would              pass without testing anything"
        );
        source.replace_range(open..=close, &rewritten);
        return name.clone();
    }
    panic!("no #[panic_handler] reaching the emitter was found to mutate");
}

#[test]
#[should_panic(expected = "must reach the capture emitter")]
fn deleting_one_panic_handlers_emit_call_would_be_caught() {
    // An in-memory mutation: the files on disk are untouched.
    let mut sources = kernel_sources();
    mutate_first_capturing_handler(&mut sources, |body| {
        let at = body.find(EMIT_CALL).expect("checked by the helper");
        let end = body[at..].find(';').expect("a statement terminator") + at + 1;
        let mut mutated = body.to_string();
        mutated.replace_range(at..end, "");
        mutated
    });
    assert_every_live_handler_captures(&panic_handlers_in(&sources));
}

#[test]
#[should_panic(expected = "panic-handler stubs under")]
fn reducing_a_live_panic_handler_to_a_stub_would_be_caught() {
    // The other way a handler can stop capturing: not by losing the call but
    // by losing the body. The anchored counts are what notices.
    let mut sources = kernel_sources();
    for (name, source) in sources.iter_mut() {
        if name.ends_with("main_aarch64.rs") {
            let mask = code_mask(source);
            let at = source
                .find("#[panic_handler]")
                .expect("main_aarch64.rs carries a panic handler");
            let (open, close) = block_span(source, &mask, at).expect("a body");
            source.replace_range(open..=close, "{ loop {} }");
        }
    }
    assert_every_live_handler_captures(&panic_handlers_in(&sources));
}

// ---------------------------------------------------------------------------
// Ordering: the capture happens before the thing that ends the boot
// ---------------------------------------------------------------------------

/// Constructs that end a panic handler. Whatever a handler reaches for, the
/// capture has to be on the wire first: `exit_qemu` ends the QEMU process at
/// that line, and the halt loops do not return.
const TERMINATORS: [(&str, &str); 4] = [
    ("exit_qemu", "ends the QEMU process, so nothing after it is emitted"),
    ("\"wfi\"", "never returns"),
    ("hlt()", "never returns"),
    ("test_panic_handler(", "does not return"),
];

fn assert_capture_precedes_every_terminator(handlers: &[PanicHandler]) {
    for handler in handlers.iter().filter(|h| !h.is_stub()) {
        let emit_at = handler
            .code
            .find(EMIT_CALL)
            .unwrap_or_else(|| panic!("{}: no capture call to order", handler.file));
        for (needle, why) in TERMINATORS {
            let Some(term_at) = handler.code.find(needle) else {
                continue;
            };
            assert!(
                emit_at < term_at,
                "{}: the capture is emitted AFTER `{needle}`, which {why}. The record \
                 would never reach the wire.",
                handler.file
            );
        }
        // And after the banner, where there is one: a reader wants the panic
        // message immediately above the state that explains it.
        if let Some(banner_at) = handler.code.find("serial_println!") {
            assert!(
                banner_at < emit_at,
                "{}: the capture is emitted BEFORE the panic banner. PR-4 places it \
                 after deliberately -- see the round doc -- so the message and the \
                 state read as one block.",
                handler.file
            );
        }
    }
}

#[test]
fn the_capture_is_emitted_before_whatever_ends_the_panic_handler() {
    assert_capture_precedes_every_terminator(&panic_handlers());
}

#[test]
#[should_panic(expected = "the capture is emitted AFTER")]
fn moving_the_capture_after_exit_qemu_would_be_caught() {
    let mut sources = kernel_sources();
    for (name, source) in sources.iter_mut() {
        if name.ends_with("kernel/src/main.rs") {
            // Reorder in memory: drop the call where it is and re-insert it
            // below the `exit_qemu` block.
            let call = "kernel::capture::emit(kernel::capture::Edge::Panic, panic_line, panic_column);";
            assert!(source.contains(call), "main.rs no longer carries the call this leg reorders");
            *source = source.replacen(call, "", 1);
            *source = source.replacen(
                "    // Disable interrupts and halt",
                &format!("    {call}\n    // Disable interrupts and halt"),
                1,
            );
        }
    }
    assert_capture_precedes_every_terminator(&panic_handlers_in(&sources));
}

// ---------------------------------------------------------------------------
// The aarch64 fatal postmortem's section 7
// ---------------------------------------------------------------------------

/// The `(index, body)` of each `dump_fatal_postmortem_section` closure in
/// `exception.rs`, in source order.
fn fatal_sections_in(source: &str) -> Vec<(usize, String)> {
    let mask = code_mask(source);
    let mut sections = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = source[cursor..].find("dump_fatal_postmortem_section(cpu_id,") {
        let at = cursor + offset;
        cursor = at + 1;
        if !mask[at] {
            continue;
        }
        let tail = &source[at..];
        let digits: String = tail["dump_fatal_postmortem_section(cpu_id,".len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let index: usize = digits.parse().unwrap_or_else(|_| {
            panic!("could not read a section index at byte {at} of {EXCEPTION_SOURCE}")
        });
        let (open, close) = block_span(source, &mask, at).unwrap_or_else(|| {
            panic!("section {index} in {EXCEPTION_SOURCE} has no closure body")
        });
        sections.push((index, source[open..=close].to_string()));
    }
    sections
}

fn assert_only_section_seven_captures(source: &str) {
    let sections = fatal_sections_in(source);
    assert_eq!(
        sections.len(),
        FATAL_SECTION_COUNT,
        "expected {FATAL_SECTION_COUNT} claimed postmortem sections in \
         {EXCEPTION_SOURCE}, found {}. The claim bitmap and this suite's \
         expectations are both written against that count.",
        sections.len()
    );
    let mut indices: Vec<usize> = sections.iter().map(|(index, _)| *index).collect();
    indices.sort_unstable();
    assert_eq!(
        indices,
        (0..FATAL_SECTION_COUNT).collect::<Vec<_>>(),
        "the postmortem's section indices are not 0..{FATAL_SECTION_COUNT} exactly; a \
         duplicated index would make two sections share one claim bit"
    );

    for (index, body) in &sections {
        if *index == CAPTURE_SECTION {
            assert!(
                body.contains(EMIT_CALL),
                "section {CAPTURE_SECTION} of the aarch64 fatal postmortem must reach \
                 the capture emitter; without it an EL1 fault dumps registers and a \
                 raw ring and no decoded, bracketed record:\n{body}"
            );
            assert!(
                body.contains("Edge::Fault"),
                "the postmortem's capture must carry Edge::Fault, or the `edge=` field \
                 names the wrong edge"
            );
            let emit_at = body.find(EMIT_CALL).expect("checked above");
            let dump_at = body.find("dump_all_buffers()").unwrap_or_else(|| {
                panic!(
                    "section {CAPTURE_SECTION} must KEEP dump_all_buffers(). PR-4 adds \
                     the bounded capture beside it rather than replacing it: \
                     `[BXCAP:EV]` carries at most 32 events from one CPU where that \
                     dump carries up to 1024 from each of 8, and narrowing the \
                     evidence on the one path that has the wide version is the \
                     opposite of the point:\n{body}"
                )
            });
            assert!(
                emit_at < dump_at,
                "the bounded, self-bracketing capture must be emitted BEFORE the \
                 unbounded dump, so a host-side kill that truncates the section still \
                 leaves a complete record"
            );
        } else {
            assert!(
                !body.contains(EMIT_CALL),
                "section {index} of the aarch64 fatal postmortem emits a capture. \
                 PR-4 wires section {CAPTURE_SECTION} alone and leaves 0..6 untouched; \
                 a second capture per fault would interleave two BEGIN/END brackets \
                 into one dump:\n{body}"
            );
        }
    }
}

#[test]
fn the_aarch64_fatal_postmortem_captures_in_section_seven_and_nowhere_else() {
    assert_only_section_seven_captures(&read(EXCEPTION_SOURCE));
}

#[test]
#[should_panic(expected = "must reach the capture emitter")]
fn deleting_the_postmortems_capture_would_be_caught() {
    let mutated = read(EXCEPTION_SOURCE)
        .replace("crate::capture::emit(crate::capture::Edge::Fault, esr, far);", "");
    assert_only_section_seven_captures(&mutated);
}

#[test]
#[should_panic(expected = "must KEEP dump_all_buffers()")]
fn replacing_the_wide_ring_dump_with_the_capture_would_be_caught() {
    let mutated = read(EXCEPTION_SOURCE).replace("crate::tracing::dump_all_buffers();", "");
    assert_only_section_seven_captures(&mutated);
}

// ---------------------------------------------------------------------------
// The oracle feature, and the one handler no profile compiles
// ---------------------------------------------------------------------------

/// The `.sh` files under `docker/qemu/`, recursively.
fn gate_scripts() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("docker/qemu must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sh") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&repo_path(GATE_DIR), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable script");
            (name, body)
        })
        .collect()
}

#[test]
fn the_panic_oracle_is_feature_gated_and_no_gate_builds_it() {
    let manifest = read("kernel/Cargo.toml");
    assert!(
        manifest.contains("capture_panic_oracle = [\"boot_tests\"]"),
        "capture_panic_oracle must ride boot_tests and must not be implied by it"
    );
    assert!(
        !manifest.contains("boot_tests = [\"capture_panic_oracle\"]"),
        "boot_tests must not pull in capture_panic_oracle; that would panic every \
         gated boot at 3 seconds"
    );

    // Its only trigger is behind the feature.
    let irq = read("kernel/src/tracing/providers/irq.rs");
    let trigger = irq
        .find("capture_oracle::observe")
        .unwrap_or_else(|| panic!("the panic oracle must be fired from trace_timer_tick"));
    let preceding = &irq[..trigger];
    let cfg_at = preceding
        .rfind("#[cfg(feature = \"capture_panic_oracle\")]")
        .unwrap_or_else(|| {
            panic!("the panic-oracle trigger must be behind #[cfg(feature = \"capture_panic_oracle\")]")
        });
    assert!(
        preceding[cfg_at..].lines().count() <= 5,
        "the capture_panic_oracle cfg must guard the trigger line itself, not something \
         several lines above it"
    );

    // The oracle module itself is behind the same cfg, so a build without the
    // feature does not compile the one `panic!` this PR adds.
    let lib = read("kernel/src/lib.rs");
    let module_at = lib
        .find("pub mod capture_oracle;")
        .expect("lib.rs must declare the oracle module");
    assert!(
        lib[..module_at].ends_with("#[cfg(feature = \"capture_panic_oracle\")]\n"),
        "kernel/src/capture_oracle.rs must be declared behind its own feature"
    );

    // And no gate script builds with it. Census-anchored on the gate scripts
    // as they exist on disk, not a literal list.
    let gates = gate_scripts();
    assert!(
        gates.len() >= 20,
        "expected the tree's gate scripts under {GATE_DIR}, found {} -- a census that \
         collapsed to a handful of files is not checking the gates",
        gates.len()
    );
    for (gate, body) in &gates {
        for (lineno, line) in body.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            assert!(
                !(line.contains("--features") && line.contains("capture_panic_oracle")),
                "{gate}:{}: builds with capture_panic_oracle, which panics every boot \
                 at 3 seconds:\n  {line}",
                lineno + 1
            );
        }
    }
}

#[test]
fn the_panic_oracle_lives_outside_the_capture_directory() {
    // The emitter may not panic -- `scripts/check-critical-path-violations.sh`
    // and `tests/capture_path_lock_free_structure.rs` both forbid it under
    // `kernel/src/capture/`. The oracle's whole job is to raise one, so it is
    // a sibling of that directory rather than a member of it. This checks the
    // rule was respected rather than routed around.
    assert!(
        repo_path("kernel/src/capture_oracle.rs").is_file(),
        "kernel/src/capture_oracle.rs must exist outside kernel/src/capture/"
    );
    assert!(
        !repo_path("kernel/src/capture/capture_oracle.rs").exists(),
        "the panic oracle must not be moved under kernel/src/capture/, where a \
         `panic!` is forbidden for the reason that directory's denylist gives"
    );
    let script = read("scripts/check-critical-path-violations.sh");
    assert!(
        script.contains("'panic!'"),
        "the capture-scoped denylist must still forbid `panic!`; if it stops, the \
         reason this module lives outside that directory has gone"
    );
}

#[test]
fn the_library_panic_handler_is_uncompiled_because_the_lib_disables_its_test_target() {
    // `kernel/src/lib.rs`'s handler is `#[cfg(test)]`, and `[lib] test =
    // false` means `cargo test` does not build this crate with `cfg(test)`.
    // The handler therefore carries a capture call that no profile in this
    // tree compiles, and the round doc lists it among the things PR-4 does
    // not claim to have executed. This test is what makes that a measured
    // fact instead of a comment: should the lib become testable, it goes red
    // and the claim gets revisited.
    let manifest = read("kernel/Cargo.toml");
    let lib_at = manifest
        .find("\n[lib]")
        .expect("kernel/Cargo.toml must declare a [lib] target");
    let after = &manifest[lib_at + 1..];
    let end = after.find("\n[").unwrap_or(after.len());
    let section = &after[..end];
    assert!(
        section.contains("test = false"),
        "kernel/Cargo.toml's [lib] no longer sets `test = false`. The `#[cfg(test)]` \
         panic handler in kernel/src/lib.rs is now compiled, so PR-4's disclosure that \
         its capture call is never built is stale:\n[lib]{section}"
    );
    let lib = read("kernel/src/lib.rs");
    assert!(
        lib.contains("#[cfg(test)]\n#[panic_handler]"),
        "kernel/src/lib.rs's panic handler must stay `#[cfg(test)]`-gated; the \
         disclosure above is written against that gate"
    );
}
