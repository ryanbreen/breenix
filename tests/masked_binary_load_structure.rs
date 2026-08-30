//! Structural oracle: the boot thread never loads a test binary with
//! interrupts masked (#554, #665).
//!
//! `userspace_test::get_test_binary()` reads the BXTEST disk, and that read
//! completes on a VirtIO block interrupt. Who is allowed to wait for it depends
//! on having a scheduler context:
//!
//!   * A syscall thread may wait with interrupts masked. `Completion::
//!     wait_timeout_inner` takes the syscall sleep path, blocks the caller
//!     `BlockedOnIO` and calls `schedule_from_kernel()`, so the CPU leaves the
//!     masked context entirely and the device interrupt is served on another
//!     thread. That is why the masked loads inside `sys_exec` are not findings.
//!   * The boot thread cannot. It has no scheduler context to park in, so on
//!     x86 `VirtioBlockDevice::irq_completion_available()` - which requires a
//!     current thread or enabled interrupts - refuses the request outright, and
//!     `get_test_binary()` turns that refusal into a panic. There is no
//!     recovery and no timeout worth waiting out.
//!
//! So the rule below governs `kernel/src/main.rs`, the boot thread's own file,
//! and every masked block in it - not a chosen profile and not a list of known
//! offenders. 62af9d13 hoisted thirteen loads out of the testing profile's
//! registration window for #554; the interactive profile kept the same shape
//! for a year afterwards because no gate runs it, which is #665. The oracle
//! makes the next such divergence impossible to leave behind.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes = None;
    let mut escaped = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            } else {
                mask[index] = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth != 0 {
            mask[index] = false;
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                mask[index + 1] = false;
                block_comment_depth += 1;
                index += 2;
            } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                mask[index + 1] = false;
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(hashes) = raw_string_hashes {
            mask[index] = false;
            if byte == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                mask[index + 1..=index + hashes].fill(false);
                raw_string_hashes = None;
                index += hashes + 1;
            }
            index += 1;
            continue;
        }
        if string || character {
            mask[index] = false;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if (string && byte == b'"') || (character && byte == b'\'') {
                string = false;
                character = false;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            mask[index] = false;
            mask[index + 1] = false;
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            mask[index] = false;
            mask[index + 1] = false;
            block_comment_depth = 1;
            index += 2;
            continue;
        }
        if byte == b'r' {
            let mut quote = index + 1;
            while bytes.get(quote) == Some(&b'#') {
                quote += 1;
            }
            if bytes.get(quote) == Some(&b'"') {
                mask[index..=quote].fill(false);
                raw_string_hashes = Some(quote - index - 1);
                index = quote + 1;
                continue;
            }
        }
        if byte == b'"' {
            mask[index] = false;
            string = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            let plain_char = bytes.get(index + 2) == Some(&b'\'');
            let escaped_char =
                bytes.get(index + 1) == Some(&b'\\') && bytes.get(index + 3) == Some(&b'\'');
            if plain_char || escaped_char {
                mask[index] = false;
                character = true;
            }
        }
        index += 1;
    }
    mask
}

fn braced_block_span(source: &str, mask: &[bool], start: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let open = (start..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
    let mut depth = 0usize;
    for index in open..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

fn normalized_code(fragment: &str) -> String {
    let mask = code_mask(fragment);
    let kept: Vec<u8> = fragment
        .bytes()
        .zip(mask)
        .filter_map(|(byte, code)| code.then_some(byte))
        .collect();
    String::from_utf8_lossy(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_whitespace(fragment: &str) -> String {
    fragment
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn compact_code(fragment: &str) -> String {
    normalized_code(fragment)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}


/// The call that opens an interrupt-masked window. `arch_without_interrupts`
/// ends in the same text, so one scan finds both spellings.
const MASKING_CALL: &str = "without_interrupts(";

/// The disk-backed load that must not happen inside one on the boot thread.
const DISK_LOAD: &str = "get_test_binary(";

/// Every interrupt-masked block in `source`, deduplicated by span.
fn masked_blocks(source: &str) -> Vec<String> {
    let mask = code_mask(source);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = source[cursor..].find(MASKING_CALL) {
        let offset = cursor + found;
        cursor = offset + MASKING_CALL.len();
        if !mask[offset..cursor].iter().all(|code| *code) {
            continue;
        }
        if let Some(span) = braced_block_span(source, &mask, offset) {
            if !spans.contains(&span) {
                spans.push(span);
            }
        }
    }
    spans
        .into_iter()
        .map(|(open, close)| source[open..=close].to_string())
        .collect()
}

/// A masked block is a finding when it reaches the disk-backed load.
fn masked_block_loads_from_disk(block: &str) -> bool {
    compact_code(block).contains(DISK_LOAD)
}

#[test]
fn no_masked_block_on_the_boot_path_loads_a_test_binary() {
    let main = repo_text("kernel/src/main.rs");
    let findings: Vec<String> = masked_blocks(&main)
        .into_iter()
        .filter(|block| masked_block_loads_from_disk(block))
        .map(|block| block.lines().take(6).collect::<Vec<_>>().join("\n"))
        .collect();
    assert!(
        findings.is_empty(),
        "kernel/src/main.rs loads a test binary with interrupts masked. The boot \
         thread has no scheduler context to park in, so the VirtIO completion is \
         refused and get_test_binary() panics (#554, #665):\n\n{}",
        findings.join("\n\n")
    );
}

#[test]
fn the_boot_path_still_loads_binaries_and_still_masks() {
    let main = repo_text("kernel/src/main.rs");
    let mask = code_mask(&main);
    let loads = main
        .match_indices(DISK_LOAD)
        .filter(|(offset, _)| mask[*offset])
        .count();
    assert!(
        loads > 0,
        "kernel/src/main.rs no longer loads any test binary - the oracle polices nothing"
    );
    assert!(
        !masked_blocks(&main).is_empty(),
        "kernel/src/main.rs no longer masks interrupts anywhere - the oracle scans nothing"
    );
}

#[test]
fn the_interactive_profile_loads_before_it_masks() {
    let main = repo_text("kernel/src/main.rs");
    let compact = compact_whitespace(&main);
    let load = compact
        .find("letelf=userspace_test::get_test_binary(\"init_shell\")")
        .expect("the interactive profile no longer loads init_shell");
    let registration = compact
        .find("process::creation::create_user_process(String::from(\"init_shell\")")
        .expect("the interactive profile no longer registers init_shell");
    assert!(
        load < registration,
        "the interactive profile registers init_shell before loading it"
    );
    let masking = compact[load..registration].contains(MASKING_CALL);
    assert!(
        masking,
        "process registration is no longer inside an interrupt-masked window - \
         if that is deliberate, this oracle needs rewriting, not deleting"
    );
}

#[test]
fn the_oracle_rejects_the_pre_fix_interactive_shape() {
    // Verbatim shape of the interactive profile before #665 was fixed.
    let pre_fix = r#"
        x86_64::instructions::interrupts::without_interrupts(|| {
            use alloc::string::String;
            serial_println!("INTERACTIVE: Loading init_shell as PID 1");
            let elf = userspace_test::get_test_binary("init_shell");
            match process::creation::create_user_process(String::from("init_shell"), &elf) {
                Ok(pid) => {}
                Err(e) => {}
            }
        });
    "#;
    let blocks = masked_blocks(pre_fix);
    assert_eq!(blocks.len(), 1, "the scan missed the masked block");
    assert!(masked_block_loads_from_disk(&blocks[0]));

    // The shipped shape: load first, mask only the registration.
    let fixed = r#"
        x86_64::instructions::interrupts::enable();
        let elf = userspace_test::get_test_binary("init_shell");
        x86_64::instructions::interrupts::without_interrupts(|| {
            use alloc::string::String;
            match process::creation::create_user_process(String::from("init_shell"), &elf) {
                Ok(pid) => {}
                Err(e) => {}
            }
        });
    "#;
    let blocks = masked_blocks(fixed);
    assert_eq!(blocks.len(), 1);
    assert!(!masked_block_loads_from_disk(&blocks[0]));

    // Both spellings of the masking call are one block, not two.
    let arch_spelling = r#"
        crate::arch_without_interrupts(|| {
            let elf = userspace_test::get_test_binary("init_shell");
        });
    "#;
    let blocks = masked_blocks(arch_spelling);
    assert_eq!(blocks.len(), 1);
    assert!(masked_block_loads_from_disk(&blocks[0]));

    // A commented-out load is not a finding.
    let commented = r#"
        x86_64::instructions::interrupts::without_interrupts(|| {
            // let elf = userspace_test::get_test_binary("init_shell");
        });
    "#;
    let blocks = masked_blocks(commented);
    assert_eq!(blocks.len(), 1);
    assert!(!masked_block_loads_from_disk(&blocks[0]));
}
