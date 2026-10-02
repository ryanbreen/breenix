//! The smoke suite: a handful of trivial cases that exercise the effort-suite
//! machinery end to end (docs/suites/smoke.json).

use libbreenix::memory::{self, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};
use libbreenix::process;
use libbreenix::suite::{case, category, check, fail, skip, suite, CaseResult, Suite};
use libbreenix::time;

static SUITE: Suite = suite("smoke", "Smoke", &[
    category("process", "Process and time", &[
        case("getpid", "getpid returns a positive PID", getpid),
        case("stdout-write", "A write to stdout returns the byte count", stdout_write),
        case("clock-read", "The monotonic clock reads and does not go backwards", clock_read),
    ]),
    category("memory", "Memory", &[
        case("brk", "brk grows the program break", brk),
        case("mmap", "An anonymous private mapping can be written and read back", mmap),
    ]),
]);

fn getpid() -> CaseResult {
    let pid = process::getpid()?;
    check(pid.raw() > 0, "getpid returned 0")
}

/// The deliberate SKIP: stdout is the suite's serial channel, where only SUITE lines go.
fn stdout_write() -> CaseResult {
    skip("stdout carries the suite's own serial lines; a test write would add a stray line")
}

fn clock_read() -> CaseResult {
    let first = time::now_monotonic()?;
    let second = time::now_monotonic()?;
    check(second.as_nanos() >= first.as_nanos(), "the monotonic clock went backwards")
}

fn brk() -> CaseResult {
    let old = memory::get_brk();
    if old == 0 {
        return fail("brk(0) returned 0");
    }
    let requested = old + 4096;
    check(memory::brk(requested) >= requested && memory::get_brk() >= requested,
        "the program break did not grow by a page")
}

fn mmap() -> CaseResult {
    const LEN: usize = 4096;
    let ptr = memory::mmap(core::ptr::null_mut(), LEN, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)?;
    // SAFETY: the kernel just mapped LEN writable bytes at ptr.
    let readback = unsafe {
        for i in 0..LEN {
            ptr.add(i).write_volatile(i as u8);
        }
        (0..LEN).all(|i| ptr.add(i).read_volatile() == i as u8)
    };
    memory::munmap(ptr, LEN)?;
    check(readback, "the mapping did not read back what was written")
}

fn main() {
    SUITE.run()
}
