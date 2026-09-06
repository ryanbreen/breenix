//! #814 PR-1 / #629 structural ratchet for the x86_64 processor enumeration.
//!
//! Three properties, each of which a plausible edit could take away silently:
//!
//! 1. The `[X86_SMP_ENUM:...]` marker has exactly ONE emission site in
//!    `kernel/src`, and `docker/qemu/run-x86-smp-enum-gate.sh` pins it by
//!    shape on the `-smp 1`, `-smp 2` and `-smp 4` legs. A second emission
//!    site would make the gate's "exactly one line" count ambiguous; a gate
//!    that stopped naming the legs would stop proving that the count MOVES.
//! 2. The MADT reader allocates nothing and every walk in it is bounded by a
//!    constant, not only by a length the table itself supplies. This is the
//!    property that lets the reader run at boot on firmware-owned memory
//!    without a heap and without a malformed table wedging it.
//! 3. `Scheduler::online_cpu_count()`'s x86 arm reads the enumeration's
//!    atomic rather than the bare `MAX_CPUS` constant -- and `MAX_CPUS` on
//!    x86 is still 1, which is this PR's own boundary: the enumeration became
//!    honest, the dispatch surface did not move.
//!
//! What this file does NOT reach, stated rather than implied: it reads source
//! text. It cannot tell whether the reader is correct about a real MADT, and
//! its allocation check is a DENYLIST of spellings (the same limitation
//! `tests/dispatch_path_lock_free_structure.rs` documents for its own) -- a
//! spelling it does not list, or an allocation reached through a callee, is
//! invisible to it. The boot evidence for the reader is the gate.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

/// Every `.rs` file below `kernel/src`, as (path, text) pairs.
fn kernel_sources() -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read kernel source directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("source below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, fs::read_to_string(&path).expect("read source")));
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join("kernel/src"), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

/// A line that EMITS the marker rather than merely mentioning it: it carries
/// the marker's opening literal and is not a comment. The doc comments in
/// `smp.rs` and `scheduler.rs` name the marker in prose; those are mentions.
fn is_marker_emission_line(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") || code.starts_with("*") {
        return false;
    }
    code.contains("[X86_SMP_ENUM:")
}

/// The body of `fn <name>(` in `source`, terminated by the closing brace at
/// the signature line's own indentation.
fn function_body(source: &str, name: &str) -> String {
    let needle = format!("fn {name}(");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no `{needle}` in the source under test"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let terminator = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == terminator)
        .unwrap_or_else(|| panic!("no terminator for `{needle}`"))
        + start
        + 1;
    lines[start..end].join("\n")
}

#[test]
fn the_enumeration_marker_has_exactly_one_emission_site() {
    let emissions: Vec<String> = kernel_sources()
        .into_iter()
        .flat_map(|(path, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| is_marker_emission_line(line))
                .map(|(index, line)| format!("{path}:{}: {}", index + 1, line.trim()))
                .collect::<Vec<String>>()
        })
        .collect();

    assert_eq!(
        emissions.len(),
        1,
        "expected exactly one [X86_SMP_ENUM:] emission site under kernel/src, found {}:\n{}",
        emissions.len(),
        emissions.join("\n")
    );
    assert!(
        emissions[0].starts_with("kernel/src/arch_impl/x86_64/smp.rs:"),
        "the enumeration marker must be emitted from the module that owns the enumeration, found: {}",
        emissions[0]
    );
}

#[test]
fn the_gate_pins_the_marker_on_three_smp_legs() {
    let gate = read("docker/qemu/run-x86-smp-enum-gate.sh");

    assert!(
        gate.contains("X86_SMP_ENUM"),
        "the SMP enumeration gate must pin the marker it exists to score"
    );
    assert!(
        gate.contains("madt_cpus=${leg}") && gate.contains("online=1"),
        "the gate must assert madt_cpus against the leg's own -smp value and online=1"
    );
    assert!(
        gate.contains("SMP_LEGS=(1 2 4)"),
        "the gate's default legs must be 1, 2 and 4: a single-leg gate cannot tell a \
         real enumeration from a hardcoded 1"
    );

    let smp_lines: Vec<&str> = gate
        .lines()
        .filter(|line| {
            let code = line.trim_start();
            !code.starts_with('#') && code.contains("-smp ")
        })
        .collect();
    assert!(
        smp_lines.iter().any(|line| line.contains("-smp \"$smp\"")),
        "the gate's QEMU line must take its -smp value from the leg variable, not a literal:\n{}",
        smp_lines.join("\n")
    );
    assert!(
        gate.contains("-accel tcg,thread=multi"),
        "the multi-vCPU legs must run under MTTCG"
    );
}

#[test]
fn the_madt_reader_allocates_nothing() {
    let acpi = read("kernel/src/arch_impl/x86_64/acpi.rs");

    // The alloc surface a no_std kernel can reach. A denylist sees only what
    // it lists; see this file's header for that limitation.
    let denied = [
        "alloc::",
        "Vec<",
        "vec![",
        "Box<",
        "Box::",
        "String",
        "format!",
        "to_string",
        "to_vec",
        "collect()",
        "BTreeMap",
        "VecDeque",
    ];
    for (index, line) in acpi.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with('*') {
            continue;
        }
        for spelling in denied {
            assert!(
                !code.contains(spelling),
                "the MADT reader must not allocate; `{spelling}` at \
                 kernel/src/arch_impl/x86_64/acpi.rs:{}: {}",
                index + 1,
                line.trim()
            );
        }
    }

    // It must also not take a lock: it runs once at boot, but the property is
    // what makes the module safe to call from anywhere later.
    assert!(
        !acpi.contains(".lock()"),
        "the MADT reader must not take a lock"
    );
}

#[test]
fn every_walk_in_the_madt_reader_is_bounded_by_a_constant() {
    let acpi = read("kernel/src/arch_impl/x86_64/acpi.rs");

    // Each bound is checked where it binds, not merely somewhere in the file:
    // a constant that is declared and unused bounds nothing.
    let entry_walk = function_body(&acpi, "census_of_madt");
    let while_lines: Vec<&str> = entry_walk
        .lines()
        .filter(|line| line.trim_start().starts_with("while "))
        .collect();
    assert_eq!(
        while_lines.len(),
        1,
        "expected exactly one entry-walk loop in census_of_madt:\n{}",
        while_lines.join("\n")
    );
    assert!(
        while_lines[0].contains("MAX_MADT_ENTRIES"),
        "the MADT entry walk must be capped by a constant iteration bound, not only by \
         the table's own declared length: {}",
        while_lines[0].trim()
    );
    assert!(
        entry_walk.contains("MADT_MIN_ENTRY_LENGTH"),
        "the MADT entry walk must refuse an entry length below the ACPI minimum -- that \
         is the shape that would otherwise not advance the cursor"
    );

    let root_walk = function_body(&acpi, "read_madt");
    assert!(
        root_walk.contains("MAX_ROOT_ENTRIES"),
        "the RSDT/XSDT walk must be capped by a constant entry bound"
    );

    let checksum = function_body(&acpi, "checksum_ok");
    assert!(
        checksum.contains("MAX_TABLE_LENGTH"),
        "the checksum walk must refuse a table longer than the reader's constant bound"
    );

    let readable = function_body(&acpi, "readable");
    assert!(
        readable.contains("PHYS_READ_CEILING") && readable.contains("checked_add"),
        "every physical read must be bounded below the ceiling, with the addition \
         checked: {readable}"
    );
}

#[test]
fn the_x86_online_count_reads_the_enumeration_and_max_cpus_is_still_one() {
    let scheduler = read("kernel/src/task/scheduler.rs");
    let online = function_body(&scheduler, "online_cpu_count");

    assert!(
        online.contains("crate::arch_impl::x86_64::smp::cpus_online()"),
        "x86's online_cpu_count() must read the enumeration's atomic (#629):\n{online}"
    );
    assert!(
        online.contains("crate::arch_impl::aarch64::smp::cpus_online()"),
        "the aarch64 arm must be unchanged:\n{online}"
    );

    // The boundary this PR does not cross. If MAX_CPUS moves off 1 on x86,
    // placement can index a CPU that nothing runs on, which is the failure
    // #629's body describes -- and this ratchet's message is where a future
    // PR-6 should look first.
    let max_cpus: Vec<&str> = scheduler
        .lines()
        .filter(|line| line.contains("const MAX_CPUS: usize"))
        .collect();
    assert_eq!(
        max_cpus.len(),
        2,
        "expected one MAX_CPUS per architecture:\n{}",
        max_cpus.join("\n")
    );
    assert!(
        max_cpus.iter().any(|line| line.contains("= 1;")),
        "x86's MAX_CPUS is still 1 in this PR; raising it needs per-CPU descriptor \
         tables, stacks and dispatch first (#814 PR-4/PR-6):\n{}",
        max_cpus.join("\n")
    );
}

/// ANTI-VACUITY: the two predicates this file leans on must fire on the real
/// shapes they claim to, in both directions. Without this, an emission-site
/// census that matched nothing and a function-body reader that returned an
/// empty string would both report green.
#[test]
fn the_predicates_are_not_vacuous() {
    assert!(is_marker_emission_line(
        r#"        "[X86_SMP_ENUM:madt_cpus={}:enabled={}]","#
    ));
    assert!(!is_marker_emission_line(
        "    // emits [X86_SMP_ENUM:...] once per boot"
    ));
    assert!(!is_marker_emission_line("    //! [X86_SMP_ENUM:] marker"));
    assert!(!is_marker_emission_line("    log::info!(\"unrelated\");"));

    let sample = "fn outer(a: u32) -> u32 {\n    let x = 1;\n    x\n}\nfn after() {}\n";
    let body = function_body(sample, "outer");
    assert!(body.contains("let x = 1;"), "body was: {body}");
    assert!(!body.contains("fn after"), "body ran past its terminator: {body}");
    assert!(body.ends_with('}'), "body must include its terminator: {body}");

    // The emission census must actually find the one real site, so a rename of
    // the marker cannot leave this file quietly passing on an empty set.
    let sources = kernel_sources();
    assert!(
        sources.len() > 100,
        "the kernel source walk found only {} files",
        sources.len()
    );
    assert!(
        sources
            .iter()
            .any(|(path, _)| path == "kernel/src/arch_impl/x86_64/smp.rs"),
        "the walk must reach the module that emits the marker"
    );
}
