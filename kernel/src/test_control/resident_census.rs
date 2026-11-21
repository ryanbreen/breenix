//! A two-sample flight recorder for the test runner's GUI Apps MEMORY verdict.
//! Sample without userspace writes, allocations, waits for locks or serial output.

mod records;

use gpu_abi::test_control::resident_census as op;
use records::{Census, Pages};
use spin::Mutex;

use crate::memory::frame_allocator::{resident_frame_owner, FrameOwner};
use crate::process::{ProcessId, ProcessManager};
use crate::syscall::errno::{EAGAIN, EINVAL, EPERM};
use crate::syscall::SyscallResult;

struct Pair {
    pid: u64,
    before: Option<Census>,
    after: Option<Census>,
}

impl Pair {
    const EMPTY: Self = Self { pid: 0, before: None, after: None };
}

static SAMPLES: Mutex<Pair> = Mutex::new(Pair::EMPTY);

// Charge the full owned table count before walking any of a process's descriptors.
// This bounds the work while PROCESS_MANAGER and the local interrupt mask are held.
const TABLE_FRAME_BUDGET: usize = 256;

fn sample(manager: &ProcessManager) -> Census {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::PageTableFlags;
    #[cfg(target_arch = "x86_64")]
    use x86_64::structures::paging::PageTableFlags;

    let mut census = Census::new();
    let mut table_budget = TABLE_FRAME_BUDGET;
    for (pid, process) in manager.iter_processes() {
        // Shared-VM rows have no owned page table. Count that address space once,
        // under its owner, and omit rows that have already given up their memory.
        let Some(table) = process.page_table.as_ref() else { continue };
        if !census.has_room() {
            break;
        }
        let tables = table.table_frames();
        if tables > table_budget
            || crate::memory::fork_copy::copying(table.level_4_frame().start_address().as_u64())
        {
            census.incomplete = true;
            continue;
        }
        table_budget -= tables;
        let mut pages = Pages { user: 0, stack: 0, tables: tables as u32 };
        table.walk_owned_mapped_pages(|_, physical, flags| {
            if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                match resident_frame_owner(physical) {
                    Some(FrameOwner::UserPages) => pages.user += 1,
                    Some(FrameOwner::UserStacks) => pages.stack += 1,
                    _ => {}
                }
            }
        });
        let _ = census.push(pid.as_u64(), &process.name, pages);
    }
    census
}

pub(super) fn control(action: u64, len: u64) -> SyscallResult {
    if len != 0 || !matches!(action, op::BEFORE | op::AFTER | op::REPORT | op::DISCARD) {
        return super::err(EINVAL);
    }
    if matches!(action, op::REPORT | op::DISCARD) {
        return drain(action == op::REPORT);
    }

    let Some(guard) = crate::process::try_manager() else { return super::err(EAGAIN) };
    let Some((pid, manager)) = crate::task::thread::current_owner_pid()
        .and_then(|pid| guard.as_ref().map(|manager| (pid, manager)))
    else {
        return super::err(EPERM);
    };
    if !manager.get_process(ProcessId::new(pid)).is_some_and(|p| p.cred.euid == 0) {
        return super::err(EPERM);
    }
    let Some(mut samples) = SAMPLES.try_lock() else { return super::err(EAGAIN) };
    if action == op::BEFORE {
        *samples = Pair { pid, before: Some(sample(manager)), after: None };
    } else {
        if samples.pid != pid {
            *samples = Pair { pid, ..Pair::EMPTY };
        }
        samples.after = Some(sample(manager));
    }
    SyscallResult::Ok(0)
}

fn drain(report: bool) -> SyscallResult {
    if !super::caller_is_root() {
        return super::err(EPERM);
    }
    let Some(pid) = crate::task::thread::current_owner_pid() else { return super::err(EPERM) };
    let pair = crate::arch_without_interrupts(|| {
        let mut samples = SAMPLES.lock();
        if samples.pid == pid {
            core::mem::replace(&mut *samples, Pair::EMPTY)
        } else {
            Pair::EMPTY
        }
    });
    if report {
        // The caller reports missing sides with the result of its sample attempts.
        for (phase, census) in [("before", pair.before), ("after", pair.after)] {
            if let Some(census) = census {
                census.report(phase, |line| crate::serial_println!("{}", line));
            }
        }
    }
    SyscallResult::Ok(0)
}
