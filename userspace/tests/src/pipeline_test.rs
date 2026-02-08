//! Pipeline Parsing Unit Tests (std version)
//!
//! Tests the pipeline parsing functions from the init_shell:
//! - contains_pipe() - Detects pipe character in input
//! - trim() - Trims leading/trailing whitespace
//! - split_pipeline() - Splits input into pipeline commands
//! - is_background_command() - Detects background operator (&)
//! - strip_background_operator() - Removes trailing &

// ============================================================================
// Copied from init_shell.rs - the functions under test
// ============================================================================

/// Maximum number of commands in a pipeline
const MAX_PIPELINE_COMMANDS: usize = 8;

/// A parsed command in a pipeline
#[derive(Clone, Copy)]
struct PipelineCommand<'a> {
    /// The command name (first word)
    name: &'a str,
    /// The full command string including arguments
    full: &'a str,
}

/// Check if the input contains a pipe character
fn contains_pipe(s: &str) -> bool {
    for c in s.as_bytes() {
        if *c == b'|' {
            return true;
        }
    }
    false
}

/// Trim leading and trailing whitespace from a string
fn trim(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();

    // Trim leading whitespace
    while start < end && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }

    // Trim trailing whitespace
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t' || bytes[end - 1] == 0)
    {
        end -= 1;
    }

    std::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

/// Check if a command should be run in the background (ends with &)
fn is_background_command(input: &str) -> bool {
    let trimmed = trim(input);
    trimmed.ends_with('&')
}

/// Strip the background operator from a command
fn strip_background_operator(input: &str) -> &str {
    let trimmed = trim(input);
    if trimmed.ends_with('&') {
        trim(&trimmed[..trimmed.len() - 1])
    } else {
        trimmed
    }
}

/// Split a string by pipe character, returning up to MAX_PIPELINE_COMMANDS segments.
/// Each segment is trimmed of whitespace.
/// Returns the number of commands found.
fn split_pipeline<'a>(
    input: &'a str,
    commands: &mut [PipelineCommand<'a>; MAX_PIPELINE_COMMANDS],
) -> usize {
    let bytes = input.as_bytes();
    let mut count = 0;
    let mut start = 0;

    for (i, &c) in bytes.iter().enumerate() {
        if c == b'|' {
            if count < MAX_PIPELINE_COMMANDS {
                let segment = trim(&input[start..i]);
                if !segment.is_empty() {
                    // Extract command name (first word)
                    let name_end = segment
                        .as_bytes()
                        .iter()
                        .position(|&ch| ch == b' ')
                        .unwrap_or(segment.len());
                    commands[count] = PipelineCommand {
                        name: &segment[..name_end],
                        full: segment,
                    };
                    count += 1;
                }
            }
            start = i + 1;
        }
    }

    // Handle the last segment after the final pipe (or entire string if no pipe)
    if count < MAX_PIPELINE_COMMANDS && start < bytes.len() {
        let segment = trim(&input[start..]);
        if !segment.is_empty() {
            let name_end = segment
                .as_bytes()
                .iter()
                .position(|&ch| ch == b' ')
                .unwrap_or(segment.len());
            commands[count] = PipelineCommand {
                name: &segment[..name_end],
                full: segment,
            };
            count += 1;
        }
    }

    count
}

// ============================================================================
// Test infrastructure
// ============================================================================

fn assert_true(condition: bool, msg: &str) {
    if !condition {
        println!("FAIL: {}", msg);
        std::process::exit(1);
    }
}

fn assert_false(condition: bool, msg: &str) {
    if condition {
        println!("FAIL: {}", msg);
        std::process::exit(1);
    }
}

fn assert_eq_usize(a: usize, b: usize, msg: &str) {
    if a != b {
        println!("FAIL: {} - expected {} got {}", msg, b, a);
        std::process::exit(1);
    }
}

fn assert_eq_str(a: &str, b: &str, msg: &str) {
    if a != b {
        println!("FAIL: {} - expected '{}' got '{}'", msg, b, a);
        std::process::exit(1);
    }
}

// ============================================================================
// contains_pipe() tests
// ============================================================================

fn test_contains_pipe() {
    println!("  Testing contains_pipe()...");

    // Should detect pipe in middle
    assert_true(contains_pipe("echo | cat"), "should detect pipe in middle");

    // Should detect pipe at start
    assert_true(contains_pipe("| cat"), "should detect pipe at start");

    // Should detect pipe at end
    assert_true(contains_pipe("echo |"), "should detect pipe at end");

    // Should not detect pipe in plain command
    assert_false(contains_pipe("echo hello"), "should not detect pipe in plain command");

    // Should not detect pipe in empty string
    assert_false(contains_pipe(""), "should not detect pipe in empty string");

    // Should detect multiple pipes
    assert_true(contains_pipe("a | b | c"), "should detect multiple pipes");

    // Should detect pipe without spaces
    assert_true(contains_pipe("echo|cat"), "should detect pipe without spaces");

    println!("    contains_pipe: PASS");
}

// ============================================================================
// trim() tests
// ============================================================================

fn test_trim() {
    println!("  Testing trim()...");

    // Trim leading spaces
    assert_eq_str(trim("  hello"), "hello", "should trim leading spaces");

    // Trim trailing spaces
    assert_eq_str(trim("hello  "), "hello", "should trim trailing spaces");

    // Trim both
    assert_eq_str(trim("  hello  "), "hello", "should trim both sides");

    // No trimming needed
    assert_eq_str(trim("hello"), "hello", "should handle no trimming needed");

    // Empty string
    assert_eq_str(trim(""), "", "should handle empty string");

    // Only spaces
    assert_eq_str(trim("   "), "", "should handle only spaces");

    // Tabs
    assert_eq_str(trim("\thello\t"), "hello", "should trim tabs");

    // Mixed whitespace
    assert_eq_str(trim(" \t hello \t "), "hello", "should trim mixed whitespace");

    // Preserve internal spaces
    assert_eq_str(trim("  hello world  "), "hello world", "should preserve internal spaces");

    println!("    trim: PASS");
}

// ============================================================================
// split_pipeline() tests
// ============================================================================

fn test_split_pipeline_single() {
    println!("  Testing split_pipeline() - single command...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("echo hello", &mut cmds);

    assert_eq_usize(count, 1, "single command count");
    assert_eq_str(cmds[0].name, "echo", "single command name");
    assert_eq_str(cmds[0].full, "echo hello", "single command full");

    println!("    split_pipeline single: PASS");
}

fn test_split_pipeline_two() {
    println!("  Testing split_pipeline() - two commands...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("echo hello | cat", &mut cmds);

    assert_eq_usize(count, 2, "two commands count");
    assert_eq_str(cmds[0].name, "echo", "first command name");
    assert_eq_str(cmds[0].full, "echo hello", "first command full");
    assert_eq_str(cmds[1].name, "cat", "second command name");
    assert_eq_str(cmds[1].full, "cat", "second command full");

    println!("    split_pipeline two: PASS");
}

fn test_split_pipeline_three() {
    println!("  Testing split_pipeline() - three commands...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("ls -la | grep foo | wc -l", &mut cmds);

    assert_eq_usize(count, 3, "three commands count");
    assert_eq_str(cmds[0].name, "ls", "first command name");
    assert_eq_str(cmds[0].full, "ls -la", "first command full");
    assert_eq_str(cmds[1].name, "grep", "second command name");
    assert_eq_str(cmds[1].full, "grep foo", "second command full");
    assert_eq_str(cmds[2].name, "wc", "third command name");
    assert_eq_str(cmds[2].full, "wc -l", "third command full");

    println!("    split_pipeline three: PASS");
}

fn test_split_pipeline_whitespace() {
    println!("  Testing split_pipeline() - whitespace handling...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("  echo  |  cat  ", &mut cmds);

    assert_eq_usize(count, 2, "whitespace-padded count");
    assert_eq_str(cmds[0].name, "echo", "first command name after trim");
    assert_eq_str(cmds[0].full, "echo", "first command full after trim");
    assert_eq_str(cmds[1].name, "cat", "second command name after trim");
    assert_eq_str(cmds[1].full, "cat", "second command full after trim");

    println!("    split_pipeline whitespace: PASS");
}

fn test_split_pipeline_empty_segments() {
    println!("  Testing split_pipeline() - empty segments...");

    // Empty string
    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("", &mut cmds);
    assert_eq_usize(count, 0, "empty string count");

    // Only pipes - should produce no valid commands (empty segments ignored)
    let count2 = split_pipeline("||", &mut cmds);
    assert_eq_usize(count2, 0, "only pipes count");

    // Spaces around pipes only
    let count3 = split_pipeline("  |  |  ", &mut cmds);
    assert_eq_usize(count3, 0, "spaces around pipes only count");

    println!("    split_pipeline empty segments: PASS");
}

fn test_split_pipeline_max_commands() {
    println!("  Testing split_pipeline() - maximum commands...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    // Test with exactly MAX_PIPELINE_COMMANDS (8) commands
    let count = split_pipeline("a | b | c | d | e | f | g | h", &mut cmds);
    assert_eq_usize(count, 8, "max commands count");

    // Verify all commands are captured
    assert_eq_str(cmds[0].name, "a", "cmd 0");
    assert_eq_str(cmds[7].name, "h", "cmd 7");

    // Test with more than MAX_PIPELINE_COMMANDS - should stop at max
    let count2 = split_pipeline("a | b | c | d | e | f | g | h | i | j", &mut cmds);
    assert_eq_usize(count2, 8, "over max commands count");

    println!("    split_pipeline max commands: PASS");
}

fn test_split_pipeline_no_spaces() {
    println!("  Testing split_pipeline() - no spaces around pipes...");

    let mut cmds = [PipelineCommand { name: "", full: "" }; MAX_PIPELINE_COMMANDS];
    let count = split_pipeline("echo|cat|wc", &mut cmds);

    assert_eq_usize(count, 3, "no spaces count");
    assert_eq_str(cmds[0].name, "echo", "first no-space name");
    assert_eq_str(cmds[1].name, "cat", "second no-space name");
    assert_eq_str(cmds[2].name, "wc", "third no-space name");

    println!("    split_pipeline no spaces: PASS");
}

// ============================================================================
// is_background_command() tests
// ============================================================================

fn test_is_background() {
    println!("  Testing is_background_command()...");

    // Trailing & with space
    assert_true(is_background_command("hello &"), "trailing & with space is background");

    // No &
    assert_false(is_background_command("hello"), "no & is not background");

    // Trailing & without space
    assert_true(is_background_command("hello&"), "trailing & without space is background");

    // Trailing & with trailing space
    assert_true(is_background_command("hello & "), "trailing & with trailing space is background");

    // Just &
    assert_true(is_background_command("&"), "just & is background");

    // & in middle (not at end)
    assert_false(is_background_command("hello & world"), "& in middle is not background");

    // Empty string
    assert_false(is_background_command(""), "empty string is not background");

    // Only spaces
    assert_false(is_background_command("   "), "only spaces is not background");

    // Complex command with &
    assert_true(is_background_command("./script --arg=value &"), "complex command with & is background");

    println!("    is_background_command: PASS");
}

// ============================================================================
// strip_background_operator() tests
// ============================================================================

fn test_strip_background() {
    println!("  Testing strip_background_operator()...");

    // Strip & with preceding space
    assert_eq_str(strip_background_operator("hello &"), "hello", "strip trailing & with space");

    // No & to strip
    assert_eq_str(strip_background_operator("hello"), "hello", "no & to strip");

    // Strip & without preceding space
    assert_eq_str(strip_background_operator("hello&"), "hello", "strip trailing & without space");

    // Strip & with trailing space
    assert_eq_str(strip_background_operator("hello & "), "hello", "strip & with trailing space");

    // Multi-word command
    assert_eq_str(strip_background_operator("hello world &"), "hello world", "strip & from multi-word");

    // Preserve internal content
    assert_eq_str(strip_background_operator("echo 'test message' &"), "echo 'test message'", "preserve quotes");

    // Just &
    assert_eq_str(strip_background_operator("&"), "", "just & becomes empty");

    // Empty string
    assert_eq_str(strip_background_operator(""), "", "empty stays empty");

    println!("    strip_background_operator: PASS");
}

// ============================================================================
// Entry point
// ============================================================================

fn main() {
    println!("=== Pipeline Parsing Unit Tests ===");
    println!("");

    println!("Testing contains_pipe()...");
    test_contains_pipe();

    println!("Testing trim()...");
    test_trim();

    println!("Testing split_pipeline()...");
    test_split_pipeline_single();
    test_split_pipeline_two();
    test_split_pipeline_three();
    test_split_pipeline_whitespace();
    test_split_pipeline_empty_segments();
    test_split_pipeline_max_commands();
    test_split_pipeline_no_spaces();

    println!("Testing is_background_command()...");
    test_is_background();

    println!("Testing strip_background_operator()...");
    test_strip_background();

    println!("");
    println!("=== All pipeline parsing tests passed ===");
    println!("PIPELINE_TEST_PASSED");

    std::process::exit(0);
}
