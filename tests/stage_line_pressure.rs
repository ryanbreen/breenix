//! Every line a boot-path stage depends on survives log pressure.
//!
//! claim-lint:ok: tests/stage_line_pressure.rs exercises marker retention under ring pressure.
//! The x86 kernel log goes through a fixed ring (`kernel/src/log_ring.rs`):
//! a line logged where the caller cannot wait on the UART is appended and left
//! for a later writer, and an ordinary line that does not fit is dropped. This
//! test compiles that ring and the kernel's stage-line scanner from their
//! sources, reads stages from both boot-path files the way the kernel
//! build does (`kernel/stage_markers.rs`), and logs each stage's marker into a
//! ring already full of ordinary lines and of other stage lines, with the ring
//! written out only when it asks for room. Each stage
//! line must come out whole, on a line of its own, and every report of
//! dropped bytes must be whole too.

#[path = "../kernel/src/log_ring.rs"]
mod log_ring;
#[path = "../kernel/src/stage_lines.rs"]
mod stage_lines;
#[path = "../kernel/stage_markers.rs"]
mod stage_markers;

use log_ring::LogRing;
use std::path::Path;

/// A small ring, so a few hundred lines put it under pressure.
const RING: usize = 4096;

fn markers() -> Vec<String> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = stage_markers::sources(repo);
    assert!(sources.contains(&repo.join("docs/boot-path.json")));
    assert!(sources.contains(&repo.join("docs/metal/boot-path.json")));
    for source in sources {
        assert!(source.exists(), "{} is missing", source.display());
    }
    let markers = stage_markers::boot_path_markers(repo).expect("resolve the boot-path stage markers");
    for metal in ["[console] uart 0x3f8 source=", "[apic] handoff base=", "[metal] kernel idle"] {
        assert!(markers.iter().any(|m| m == metal), "missing metal marker {metal}");
    }
    for host in ["console-ready", "power-on", "firmware-text", "post", "image-complete"] {
        assert!(!markers.iter().any(|m| m == host), "host event {host} became a serial marker");
    }
    markers
}

/// Log `line` as the kernel does. When the ring asks for room, write out its
/// oldest bytes until it has that much, as the kernel's writer does.
fn log(ring: &mut LogRing<RING>, out: &mut Vec<u8>, markers: &[&str], line: &str) {
    loop {
        match ring.append(format_args!("{line}\n"), markers) {
            Ok(appended) => {
                assert_eq!(appended, ring.pushed());
                return;
            }
            Err(needed) => {
                assert!(
                    needed <= RING,
                    "the ring asked for {needed} bytes, more than it holds"
                );
                while RING - ring.len() < needed {
                    let mut chunk = [0u8; 16];
                    let n = ring.pop(&mut chunk);
                    assert!(n != 0, "the ring asked for room it could not make");
                    out.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
}

fn drain(ring: &mut LogRing<RING>, out: &mut Vec<u8>) {
    let mut chunk = [0u8; 16];
    loop {
        let n = ring.pop(&mut chunk);
        if n == 0 {
            return;
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

/// An ordinary line, shaped like the testing kernel's syscall logging.
fn filler(n: usize) -> String {
    format!(
        "[ INFO] kernel::syscall::memory: sys_brk: thread={n} pid=ProcessId({n}) addr=0x4009c000 heap_start=0x4001c000"
    )
}

fn stage_line(marker: &str) -> String {
    format!("[ INFO] kernel: {marker} (logged under pressure)")
}

fn is_drop_report(line: &str) -> bool {
    line.strip_prefix("[log: ")
        .and_then(|rest| rest.strip_suffix(" bytes dropped]"))
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit()))
}

#[test]
fn every_boot_path_stage_line_survives_log_pressure() {
    let markers = markers();
    let markers: Vec<&str> = markers.iter().map(String::as_str).collect();
    assert!(
        !markers.is_empty(),
        "docs/boot-path.json lists no stage markers"
    );
    assert_eq!(stage_markers::MAX_MARKER, stage_lines::MAX_MARKER);
    assert!(
        !stage_lines::contains(&markers, filler(0).as_bytes()),
        "the filler line itself carries a stage marker"
    );

    let mut lost = Vec::new();
    let mut split = Vec::new();
    for (index, marker) in markers.iter().enumerate() {
        let mut ring = Box::new(LogRing::<RING>::new());
        let mut out = Vec::new();
        let mut logged = Vec::new();

        // Ordinary lines past what the ring holds: some are dropped.
        for n in 0..2 * RING / 64 {
            log(&mut ring, &mut out, &markers, &filler(n));
        }
        // Other stage lines, enough to use up any headroom kept for them.
        for n in 0..RING / 64 {
            let other = stage_line(markers[(index + 1 + n) % markers.len()]);
            log(&mut ring, &mut out, &markers, &other);
            logged.push(other);
        }
        let line = stage_line(marker);
        log(&mut ring, &mut out, &markers, &line);
        logged.push(line);
        for n in 0..RING / 64 {
            log(&mut ring, &mut out, &markers, &filler(n));
        }
        drain(&mut ring, &mut out);

        let text = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = text.lines().collect();
        let fillers = lines
            .iter()
            .filter(|line| line.contains("kernel::syscall::memory"))
            .count();
        assert!(
            fillers < 2 * RING / 64 + RING / 64,
            "marker {marker:?}: no ordinary line was dropped, so the ring was never under pressure"
        );
        for line in &lines {
            if line.contains("[log:") && !is_drop_report(line) {
                split.push(format!("{marker:?}: {line:?}"));
            }
        }
        for wanted in &logged {
            if !lines.contains(&wanted.as_str()) {
                lost.push(format!("{marker:?}: {wanted:?}"));
            }
        }
    }
    assert!(
        lost.is_empty() && split.is_empty(),
        "{} stage lines did not come out whole and {} drop reports were split under log pressure \
         ({} markers)\nfirst lost: {:?}\nfirst split: {:?}",
        lost.len(),
        split.len(),
        markers.len(),
        lost.first(),
        split.first()
    );
}

#[test]
fn metal_modes_resolve_only_against_the_gate_arch_catalog() {
    let repo = std::env::temp_dir().join(format!(
        "metal-marker-resolution-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let catalogs = repo.join("tools/breenix-runs/Sources/BreenixRuns/Resources");
    std::fs::create_dir_all(&catalogs).unwrap();
    std::fs::create_dir_all(repo.join("docs/metal")).unwrap();
    std::fs::write(catalogs.join("boot-stages-aarch64.json"), r#"{"stages":[
        {"name":"Shared", "marker":"arm-only-marker"},
        {"name":"ARM only", "marker":"arm-exclusive"}]}"#).unwrap();
    std::fs::write(catalogs.join("boot-stages-x86_64.json"), r#"{"stages":[
        {"name":"Main", "marker":"main-marker"},
        {"name":"Shared", "marker":"x86-only-marker"}]}"#).unwrap();
    let main = repo.join("docs/boot-path.json");
    let metal = repo.join("docs/metal/boot-path.json");
    std::fs::write(&main, r#"{"milestones":[{"id":"main","stages":{"x86_64":["Main"]}}]}"#).unwrap();
    let definition = r#"{"gateArch":"x86_64","scoring":"modes","milestones":[
        {"id":"metal","stages":{"shell":["Shared",
        {"name":"Console ready","host":"console-ready"},
        {"name":"Inline","marker":"metal-inline|metal-alternative"}]}}]}"#;
    std::fs::write(&metal, definition).unwrap();
    let markers = stage_markers::boot_path_markers(&repo).unwrap();
    for expected in ["main-marker", "x86-only-marker", "metal-inline", "metal-alternative"] {
        assert!(markers.iter().any(|m| m == expected), "missing {expected}");
    }
    for excluded in ["arm-only-marker", "console-ready"] {
        assert!(!markers.iter().any(|m| m == excluded), "unexpected {excluded}");
    }
    std::fs::write(&main, r#"{"milestones":[{"id":"main","stages":{"shell":["Shared"]}}]}"#).unwrap();
    let markers = stage_markers::boot_path_markers(&repo).unwrap();
    assert!(markers.iter().any(|m| m == "arm-only-marker"));
    assert!(markers.iter().any(|m| m == "x86-only-marker"));
    for (bad, message) in [
        (definition.replace("\"gateArch\":\"x86_64\",", ""), "missing gateArch"),
        (definition.replace("\"gateArch\":\"x86_64\"", "\"gateArch\":\"unknown\""), "unknown gateArch unknown"),
    ] {
        std::fs::write(&metal, bad).unwrap();
        assert_eq!(stage_markers::boot_path_markers(&repo).unwrap_err(),
            format!("docs/metal/boot-path.json: {message}"));
    }
    std::fs::write(&metal, definition.replace("Shared", "ARM only")).unwrap();
    assert!(stage_markers::boot_path_markers(&repo).unwrap_err().contains("not in the x86_64 catalog"));
    std::fs::write(&metal, definition.replace("metal-inline", &"x".repeat(129))).unwrap();
    assert!(stage_markers::boot_path_markers(&repo).unwrap_err().contains("longer than 128 bytes"));
    std::fs::remove_dir_all(repo).unwrap();
}

/// The console writer moves the ring a line at a time (`serial::queue_log_line`):
/// `line_len` is the oldest line's length through its line end, across the
/// ring's wrap, and the whole waiting length when no line end is waiting.
#[test]
fn line_len_is_the_oldest_lines_length_through_its_end() {
    let mut ring = Box::new(LogRing::<RING>::new());
    assert_eq!(ring.line_len(), 0);
    let mut out = Vec::new();
    // Push the ring's head near its end so the next lines wrap.
    while ring.pushed() < (RING - 40) as u64 {
        log(&mut ring, &mut out, &[], "x");
        drain(&mut ring, &mut out);
    }
    log(&mut ring, &mut out, &[], "first line that wraps the ring's end");
    log(&mut ring, &mut out, &[], "second");
    assert_eq!(ring.line_len(), "first line that wraps the ring's end\n".len());
    let mut first = vec![0u8; ring.line_len()];
    assert_eq!(ring.pop(&mut first), first.len());
    assert_eq!(first, b"first line that wraps the ring's end\n");
    assert_eq!(ring.line_len(), "second\n".len());
    let mut partial = [0u8; 3];
    ring.pop(&mut partial);
    assert_eq!(ring.line_len(), "ond\n".len());
    let mut rest = [0u8; 3];
    ring.pop(&mut rest);
    assert_eq!(ring.line_len(), 1, "the line end alone");
}
