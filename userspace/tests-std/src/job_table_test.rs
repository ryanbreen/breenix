//! JobTable Unit Tests (std version)
//!
//! Tests for the JobTable data structure and parse_job_spec function
//! used by the Breenix shell for job control.

// ============================================================================
// Test Infrastructure
// ============================================================================

/// Track test statistics
static mut TESTS_RUN: u32 = 0;
static mut TESTS_PASSED: u32 = 0;

/// Assert equality for u32
fn assert_eq_u32(actual: u32, expected: u32, msg: &str) -> bool {
    unsafe {
        TESTS_RUN += 1;
    }
    if actual == expected {
        unsafe {
            TESTS_PASSED += 1;
        }
        true
    } else {
        println!("  FAIL: {} - expected {}, got {}", msg, expected, actual);
        false
    }
}

/// Assert equality for i32
fn assert_eq_i32(actual: i32, expected: i32, msg: &str) -> bool {
    unsafe {
        TESTS_RUN += 1;
    }
    if actual == expected {
        unsafe {
            TESTS_PASSED += 1;
        }
        true
    } else {
        println!("  FAIL: {} - expected {}, got {}", msg, expected, actual);
        false
    }
}

/// Assert a boolean condition
fn assert_true(condition: bool, msg: &str) -> bool {
    unsafe {
        TESTS_RUN += 1;
    }
    if condition {
        unsafe {
            TESTS_PASSED += 1;
        }
        true
    } else {
        println!("  FAIL: {}", msg);
        false
    }
}

/// Assert Option is Some
fn assert_some<T>(opt: Option<T>, msg: &str) -> bool {
    assert_true(opt.is_some(), msg)
}

/// Assert Option is None
fn assert_none<T>(opt: Option<T>, msg: &str) -> bool {
    assert_true(opt.is_none(), msg)
}

// ============================================================================
// Copied from init_shell.rs - JobTable Implementation
// ============================================================================

/// Status of a job in the job table
#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
enum JobStatus {
    Running = 0,
    Stopped = 1,
    Done = 2,
}

/// Maximum length of command string stored in a job
const JOB_COMMAND_MAX: usize = 128;

/// A job entry representing a background or stopped process
#[derive(Clone, Copy)]
struct Job {
    /// Job ID (1-based, shown to user as [1], [2], etc.)
    id: u32,
    /// Process ID of the job
    pid: i32,
    /// Process group ID of the job
    pgid: i32,
    /// Current status of the job
    status: JobStatus,
    /// Command string stored as fixed-size buffer (no heap allocation)
    command: [u8; JOB_COMMAND_MAX],
    /// Actual length of the command string
    command_len: usize,
}

impl Job {
    /// Get the command as a string slice
    fn command_str(&self) -> &str {
        std::str::from_utf8(&self.command[..self.command_len]).unwrap_or("")
    }
}

/// Maximum number of concurrent jobs
const MAX_JOBS: usize = 16;

/// Job table tracking all background and stopped jobs
struct JobTable {
    /// Array of job slots (None = empty slot)
    jobs: [Option<Job>; MAX_JOBS],
    /// Next job ID to assign
    next_id: u32,
    /// ID of the current (most recent) job
    current: u32,
}

impl JobTable {
    /// Create a new empty job table
    const fn new() -> Self {
        const NONE: Option<Job> = None;
        JobTable {
            jobs: [NONE; MAX_JOBS],
            next_id: 1,
            current: 0,
        }
    }

    /// Add a new job to the table
    ///
    /// Returns the job ID, or 0 if the table is full
    fn add(&mut self, pid: i32, pgid: i32, command: &str) -> u32 {
        // Find an empty slot
        for slot in self.jobs.iter_mut() {
            if slot.is_none() {
                let id = self.next_id;
                self.next_id += 1;

                // Copy command into fixed buffer
                let mut cmd_buf = [0u8; JOB_COMMAND_MAX];
                let cmd_bytes = command.as_bytes();
                let cmd_len = cmd_bytes.len().min(JOB_COMMAND_MAX);
                cmd_buf[..cmd_len].copy_from_slice(&cmd_bytes[..cmd_len]);

                *slot = Some(Job {
                    id,
                    pid,
                    pgid,
                    status: JobStatus::Running,
                    command: cmd_buf,
                    command_len: cmd_len,
                });

                self.current = id;
                return id;
            }
        }
        0 // Table full
    }

    /// Find a job by its job ID
    fn find_by_id(&self, id: u32) -> Option<&Job> {
        for slot in &self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Find a job by its process ID
    fn find_by_pid(&self, pid: i32) -> Option<&Job> {
        for slot in &self.jobs {
            if let Some(job) = slot {
                if job.pid == pid {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Find a job by its process ID (mutable)
    fn find_by_pid_mut(&mut self, pid: i32) -> Option<&mut Job> {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.pid == pid {
                    return Some(job);
                }
            }
        }
        None
    }

    /// Update the status of a job by PID
    fn update_status(&mut self, pid: i32, status: JobStatus) {
        if let Some(job) = self.find_by_pid_mut(pid) {
            job.status = status;
        }
    }

    /// Remove a job by its job ID
    fn remove(&mut self, id: u32) {
        for slot in &mut self.jobs {
            if let Some(job) = slot {
                if job.id == id {
                    *slot = None;
                    return;
                }
            }
        }
    }

    /// Get the current (most recent) job
    #[allow(dead_code)]
    fn current_job(&self) -> Option<&Job> {
        self.find_by_id(self.current)
    }

    /// Count active jobs
    fn count(&self) -> usize {
        self.jobs.iter().filter(|slot| slot.is_some()).count()
    }
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

/// Simulated get_current_job_id for parse_job_spec testing
static mut TEST_CURRENT_JOB: u32 = 0;

fn get_current_job_id() -> u32 {
    unsafe { TEST_CURRENT_JOB }
}

fn set_current_job_id(id: u32) {
    unsafe {
        TEST_CURRENT_JOB = id;
    }
}

/// Parse a job specification string into a job ID
///
/// Accepts formats:
/// - "%1" or "%2" etc. (job ID with % prefix)
/// - "1" or "2" etc. (bare job ID)
/// - "" (empty string, returns current job)
///
/// Returns 0 if the spec is invalid
fn parse_job_spec(spec: &str) -> u32 {
    let spec = trim(spec);

    if spec.is_empty() {
        return get_current_job_id();
    }

    // Strip leading '%' if present
    let num_str = if spec.starts_with('%') {
        &spec[1..]
    } else {
        spec
    };

    // Parse the number manually (matching the no_std implementation)
    let mut result: u32 = 0;
    for c in num_str.as_bytes() {
        if *c >= b'0' && *c <= b'9' {
            result = result * 10 + (*c - b'0') as u32;
        } else {
            return 0; // Invalid character
        }
    }

    result
}

// ============================================================================
// Test Functions
// ============================================================================

/// Test JobTable::add() - basic functionality
fn test_job_table_add() {
    println!("test_job_table_add:");

    let mut table = JobTable::new();

    // First job should have id 1
    let id1 = table.add(100, 100, "hello");
    assert_eq_u32(id1, 1, "first job should have id 1");

    // Second job should have id 2
    let id2 = table.add(101, 101, "world");
    assert_eq_u32(id2, 2, "second job should have id 2");

    // Third job should have id 3
    let id3 = table.add(102, 102, "test");
    assert_eq_u32(id3, 3, "third job should have id 3");

    // Verify count
    assert_eq_u32(table.count() as u32, 3, "should have 3 jobs");

    println!("  PASS: test_job_table_add");
}

/// Test JobTable::add() - sets status to Running
fn test_job_table_add_status() {
    println!("test_job_table_add_status:");

    let mut table = JobTable::new();
    let id = table.add(100, 100, "test_cmd");

    let job = table.find_by_id(id).unwrap();
    assert_true(
        job.status == JobStatus::Running,
        "new job should have Running status",
    );

    println!("  PASS: test_job_table_add_status");
}

/// Test JobTable::add() - command string storage
fn test_job_table_add_command() {
    println!("test_job_table_add_command:");

    let mut table = JobTable::new();
    let id = table.add(100, 100, "my_command arg1 arg2");

    let job = table.find_by_id(id).unwrap();
    assert_true(
        job.command_str() == "my_command arg1 arg2",
        "command string should be stored correctly",
    );

    println!("  PASS: test_job_table_add_command");
}

/// Test JobTable::add() - command truncation for long commands
fn test_job_table_add_command_truncation() {
    println!("test_job_table_add_command_truncation:");

    let mut table = JobTable::new();

    // Create a command longer than JOB_COMMAND_MAX (128 bytes)
    let long_command = "this_is_a_very_long_command_that_exceeds_the_maximum_buffer_size_of_128_bytes_and_should_be_truncated_to_fit_within_the_buffer_limit_properly";

    let id = table.add(100, 100, long_command);

    let job = table.find_by_id(id).unwrap();
    assert_eq_u32(
        job.command_len as u32,
        JOB_COMMAND_MAX as u32,
        "command should be truncated to max length",
    );

    // Verify the stored command is the prefix of the original
    let stored = job.command_str();
    assert_true(
        long_command.starts_with(stored),
        "truncated command should be prefix of original",
    );

    println!("  PASS: test_job_table_add_command_truncation");
}

/// Test JobTable::add() - table full returns 0
fn test_job_table_full() {
    println!("test_job_table_full:");

    let mut table = JobTable::new();

    // Fill the table (MAX_JOBS = 16)
    for i in 0..MAX_JOBS {
        let id = table.add(100 + i as i32, 100 + i as i32, "job");
        assert_true(id > 0, "should successfully add job");
    }

    assert_eq_u32(table.count() as u32, MAX_JOBS as u32, "table should be full");

    // Now adding should fail
    let overflow_id = table.add(200, 200, "overflow");
    assert_eq_u32(overflow_id, 0, "adding to full table should return 0");

    println!("  PASS: test_job_table_full");
}

/// Test JobTable::add() - updates current job
fn test_job_table_add_updates_current() {
    println!("test_job_table_add_updates_current:");

    let mut table = JobTable::new();

    let id1 = table.add(100, 100, "first");
    assert_eq_u32(table.current, id1, "current should be first job");

    let id2 = table.add(101, 101, "second");
    assert_eq_u32(table.current, id2, "current should be second job");

    let id3 = table.add(102, 102, "third");
    assert_eq_u32(table.current, id3, "current should be third job");

    println!("  PASS: test_job_table_add_updates_current");
}

/// Test JobTable::find_by_id() - existing job
fn test_find_by_id_existing() {
    println!("test_find_by_id_existing:");

    let mut table = JobTable::new();
    let id1 = table.add(100, 100, "first");
    let id2 = table.add(101, 101, "second");

    let job1 = table.find_by_id(id1);
    assert_some(job1, "should find first job");
    assert_eq_i32(job1.unwrap().pid, 100, "first job should have pid 100");

    let job2 = table.find_by_id(id2);
    assert_some(job2, "should find second job");
    assert_eq_i32(job2.unwrap().pid, 101, "second job should have pid 101");

    println!("  PASS: test_find_by_id_existing");
}

/// Test JobTable::find_by_id() - non-existent job
fn test_find_by_id_nonexistent() {
    println!("test_find_by_id_nonexistent:");

    let mut table = JobTable::new();
    table.add(100, 100, "first");

    let result = table.find_by_id(999);
    assert_none(result, "should not find non-existent job");

    let result_zero = table.find_by_id(0);
    assert_none(result_zero, "should not find job with id 0");

    println!("  PASS: test_find_by_id_nonexistent");
}

/// Test JobTable::find_by_pid() - existing job
fn test_find_by_pid_existing() {
    println!("test_find_by_pid_existing:");

    let mut table = JobTable::new();
    let id1 = table.add(100, 100, "first");
    table.add(101, 101, "second");

    let job = table.find_by_pid(100);
    assert_some(job, "should find job by pid");
    assert_eq_u32(job.unwrap().id, id1, "found job should have correct id");

    println!("  PASS: test_find_by_pid_existing");
}

/// Test JobTable::find_by_pid() - non-existent pid
fn test_find_by_pid_nonexistent() {
    println!("test_find_by_pid_nonexistent:");

    let mut table = JobTable::new();
    table.add(100, 100, "first");

    let result = table.find_by_pid(999);
    assert_none(result, "should not find non-existent pid");

    println!("  PASS: test_find_by_pid_nonexistent");
}

/// Test JobTable::update_status() - status transitions
fn test_update_status() {
    println!("test_update_status:");

    let mut table = JobTable::new();
    table.add(100, 100, "test");

    // Initial status should be Running
    let job = table.find_by_pid(100).unwrap();
    assert_true(job.status == JobStatus::Running, "initial status is Running");

    // Transition to Stopped
    table.update_status(100, JobStatus::Stopped);
    let job = table.find_by_pid(100).unwrap();
    assert_true(
        job.status == JobStatus::Stopped,
        "status should be Stopped after update",
    );

    // Transition back to Running
    table.update_status(100, JobStatus::Running);
    let job = table.find_by_pid(100).unwrap();
    assert_true(
        job.status == JobStatus::Running,
        "status should be Running after resume",
    );

    // Transition to Done
    table.update_status(100, JobStatus::Done);
    let job = table.find_by_pid(100).unwrap();
    assert_true(
        job.status == JobStatus::Done,
        "status should be Done after completion",
    );

    println!("  PASS: test_update_status");
}

/// Test JobTable::update_status() - non-existent pid is no-op
fn test_update_status_nonexistent() {
    println!("test_update_status_nonexistent:");

    let mut table = JobTable::new();
    table.add(100, 100, "test");

    // Update non-existent pid should not crash
    table.update_status(999, JobStatus::Done);

    // Original job should be unchanged
    let job = table.find_by_pid(100).unwrap();
    assert_true(
        job.status == JobStatus::Running,
        "original job status unchanged",
    );

    println!("  PASS: test_update_status_nonexistent");
}

/// Test JobTable::remove() - removes job
fn test_remove_existing() {
    println!("test_remove_existing:");

    let mut table = JobTable::new();
    let id1 = table.add(100, 100, "first");
    let id2 = table.add(101, 101, "second");

    assert_eq_u32(table.count() as u32, 2, "should have 2 jobs initially");

    table.remove(id1);

    assert_eq_u32(table.count() as u32, 1, "should have 1 job after remove");
    assert_none(table.find_by_id(id1), "removed job should not be found");
    assert_some(table.find_by_id(id2), "other job should still exist");

    println!("  PASS: test_remove_existing");
}

/// Test JobTable::remove() - non-existent job is no-op
fn test_remove_nonexistent() {
    println!("test_remove_nonexistent:");

    let mut table = JobTable::new();
    let id = table.add(100, 100, "test");

    // Remove non-existent job should not crash
    table.remove(999);

    // Original job should still exist
    assert_eq_u32(table.count() as u32, 1, "count unchanged");
    assert_some(table.find_by_id(id), "original job still exists");

    println!("  PASS: test_remove_nonexistent");
}

/// Test JobTable::remove() - can reuse slot after removal
fn test_remove_slot_reuse() {
    println!("test_remove_slot_reuse:");

    let mut table = JobTable::new();

    // Fill the table
    for i in 0..MAX_JOBS {
        table.add(100 + i as i32, 100 + i as i32, "job");
    }

    assert_eq_u32(table.count() as u32, MAX_JOBS as u32, "table is full");

    // Can't add more
    let overflow = table.add(999, 999, "overflow");
    assert_eq_u32(overflow, 0, "can't add to full table");

    // Remove one job (use the first one, id=1)
    table.remove(1);

    // Now we can add again
    let new_id = table.add(200, 200, "new_job");
    assert_true(new_id > 0, "can add after removing");

    println!("  PASS: test_remove_slot_reuse");
}

/// Test parse_job_spec() - percent prefix
fn test_parse_job_spec_percent() {
    println!("test_parse_job_spec_percent:");

    assert_eq_u32(parse_job_spec("%1"), 1, "%1 should return 1");
    assert_eq_u32(parse_job_spec("%2"), 2, "%2 should return 2");
    assert_eq_u32(parse_job_spec("%123"), 123, "%123 should return 123");
    assert_eq_u32(parse_job_spec("%42"), 42, "%42 should return 42");

    println!("  PASS: test_parse_job_spec_percent");
}

/// Test parse_job_spec() - bare numbers
fn test_parse_job_spec_bare() {
    println!("test_parse_job_spec_bare:");

    assert_eq_u32(parse_job_spec("1"), 1, "1 should return 1");
    assert_eq_u32(parse_job_spec("2"), 2, "2 should return 2");
    assert_eq_u32(parse_job_spec("123"), 123, "123 should return 123");
    assert_eq_u32(parse_job_spec("999"), 999, "999 should return 999");

    println!("  PASS: test_parse_job_spec_bare");
}

/// Test parse_job_spec() - empty string returns current job
fn test_parse_job_spec_empty() {
    println!("test_parse_job_spec_empty:");

    set_current_job_id(5);
    assert_eq_u32(parse_job_spec(""), 5, "empty string should return current job");

    set_current_job_id(0);
    assert_eq_u32(
        parse_job_spec(""),
        0,
        "empty string with no current job returns 0",
    );

    set_current_job_id(42);
    assert_eq_u32(
        parse_job_spec(""),
        42,
        "empty string should return current job 42",
    );

    println!("  PASS: test_parse_job_spec_empty");
}

/// Test parse_job_spec() - whitespace handling
fn test_parse_job_spec_whitespace() {
    println!("test_parse_job_spec_whitespace:");

    assert_eq_u32(parse_job_spec("  %1  "), 1, "should handle leading/trailing spaces");
    assert_eq_u32(parse_job_spec("\t%2\t"), 2, "should handle tabs");
    assert_eq_u32(parse_job_spec("  3  "), 3, "should handle spaces around bare number");

    println!("  PASS: test_parse_job_spec_whitespace");
}

/// Test parse_job_spec() - invalid input
fn test_parse_job_spec_invalid() {
    println!("test_parse_job_spec_invalid:");

    assert_eq_u32(parse_job_spec("abc"), 0, "letters should return 0");
    assert_eq_u32(parse_job_spec("%abc"), 0, "%abc should return 0");
    assert_eq_u32(parse_job_spec("1a2"), 0, "mixed digits and letters should return 0");
    assert_eq_u32(parse_job_spec("-1"), 0, "negative should return 0");
    assert_eq_u32(parse_job_spec("%-1"), 0, "%-1 should return 0");
    assert_eq_u32(parse_job_spec("%%1"), 0, "double percent should return 0");

    println!("  PASS: test_parse_job_spec_invalid");
}

/// Test parse_job_spec() - zero
fn test_parse_job_spec_zero() {
    println!("test_parse_job_spec_zero:");

    // "0" is technically a valid number parse, should return 0
    assert_eq_u32(parse_job_spec("0"), 0, "0 should return 0");
    assert_eq_u32(parse_job_spec("%0"), 0, "%0 should return 0");

    println!("  PASS: test_parse_job_spec_zero");
}

/// Test JobTable with mixed operations
fn test_job_table_mixed_operations() {
    println!("test_job_table_mixed_operations:");

    let mut table = JobTable::new();

    // Add some jobs
    let id1 = table.add(100, 100, "job1");
    let id2 = table.add(101, 101, "job2");
    let id3 = table.add(102, 102, "job3");

    // Stop job 2
    table.update_status(101, JobStatus::Stopped);

    // Complete job 1
    table.update_status(100, JobStatus::Done);

    // Verify states
    assert_true(
        table.find_by_id(id1).unwrap().status == JobStatus::Done,
        "job1 should be Done",
    );
    assert_true(
        table.find_by_id(id2).unwrap().status == JobStatus::Stopped,
        "job2 should be Stopped",
    );
    assert_true(
        table.find_by_id(id3).unwrap().status == JobStatus::Running,
        "job3 should still be Running",
    );

    // Remove completed job
    table.remove(id1);
    assert_eq_u32(table.count() as u32, 2, "should have 2 jobs after removing job1");

    // Resume stopped job
    table.update_status(101, JobStatus::Running);
    assert_true(
        table.find_by_id(id2).unwrap().status == JobStatus::Running,
        "job2 should be Running after resume",
    );

    println!("  PASS: test_job_table_mixed_operations");
}

/// Test Job::command_str()
fn test_job_command_str() {
    println!("test_job_command_str:");

    let mut table = JobTable::new();
    table.add(100, 100, "");
    table.add(101, 101, "single");
    table.add(102, 102, "multi word command with spaces");

    assert_true(
        table.find_by_pid(100).unwrap().command_str() == "",
        "empty command should work",
    );
    assert_true(
        table.find_by_pid(101).unwrap().command_str() == "single",
        "single word command",
    );
    assert_true(
        table.find_by_pid(102).unwrap().command_str() == "multi word command with spaces",
        "multi word command",
    );

    println!("  PASS: test_job_command_str");
}

/// Test JobTable::current_job()
fn test_current_job() {
    println!("test_current_job:");

    let mut table = JobTable::new();

    // No jobs - current should be None
    assert_none(table.current_job(), "no current job when table empty");

    let id1 = table.add(100, 100, "first");
    assert_some(table.current_job(), "should have current job after add");
    assert_eq_u32(
        table.current_job().unwrap().id,
        id1,
        "current job should be first added",
    );

    let id2 = table.add(101, 101, "second");
    assert_eq_u32(
        table.current_job().unwrap().id,
        id2,
        "current job should be most recent",
    );

    // Removing current job doesn't update current pointer (matches shell behavior)
    table.remove(id2);
    // current still points to id2, but find returns None
    assert_none(
        table.current_job(),
        "current_job returns None when current was removed",
    );

    println!("  PASS: test_current_job");
}

/// Test pgid storage in Job
fn test_job_pgid() {
    println!("test_job_pgid:");

    let mut table = JobTable::new();

    // Test that pgid is stored correctly (can differ from pid)
    table.add(100, 200, "job_with_different_pgid");

    let job = table.find_by_pid(100).unwrap();
    assert_eq_i32(job.pid, 100, "pid should be 100");
    assert_eq_i32(job.pgid, 200, "pgid should be 200");

    println!("  PASS: test_job_pgid");
}

// ============================================================================
// Main Entry Point
// ============================================================================

fn main() {
    println!("=== JobTable Unit Tests ===");
    println!();

    // JobTable::add() tests
    test_job_table_add();
    test_job_table_add_status();
    test_job_table_add_command();
    test_job_table_add_command_truncation();
    test_job_table_full();
    test_job_table_add_updates_current();

    // JobTable::find_by_id() tests
    test_find_by_id_existing();
    test_find_by_id_nonexistent();

    // JobTable::find_by_pid() tests
    test_find_by_pid_existing();
    test_find_by_pid_nonexistent();

    // JobTable::update_status() tests
    test_update_status();
    test_update_status_nonexistent();

    // JobTable::remove() tests
    test_remove_existing();
    test_remove_nonexistent();
    test_remove_slot_reuse();

    // parse_job_spec() tests
    test_parse_job_spec_percent();
    test_parse_job_spec_bare();
    test_parse_job_spec_empty();
    test_parse_job_spec_whitespace();
    test_parse_job_spec_invalid();
    test_parse_job_spec_zero();

    // Additional tests
    test_job_table_mixed_operations();
    test_job_command_str();
    test_current_job();
    test_job_pgid();

    // Print summary
    println!();
    println!("=== Test Summary ===");
    unsafe {
        let tests_run = std::ptr::addr_of!(TESTS_RUN).read_volatile();
        let tests_passed = std::ptr::addr_of!(TESTS_PASSED).read_volatile();
        println!("Tests run: {}", tests_run);
        println!("Tests passed: {}", tests_passed);

        if tests_passed == tests_run {
            println!("=== All JobTable tests passed ===");
            println!("JOB_TABLE_TEST_PASSED");
            std::process::exit(0);
        } else {
            println!("FAILED: {} tests failed", tests_run - tests_passed);
            std::process::exit(1);
        }
    }
}
