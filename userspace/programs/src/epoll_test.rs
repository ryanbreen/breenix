//! Epoll syscall test program.
//!
//! Covers basic epoll_create1/epoll_ctl/epoll_wait behavior and a fork/CoW
//! case where epoll_wait writes into a child stack buffer inherited from the
//! parent.

use libbreenix::io;
use libbreenix::process::{fork, waitpid, wexitstatus, wifexited, ForkResult};
use libbreenix::syscall::{nr, raw};
use std::process;

const EPOLLIN: u32 = 0x001;
const EPOLL_CTL_ADD: i32 = 1;

#[repr(C)]
#[cfg_attr(target_arch = "x86_64", repr(packed))]
#[derive(Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

impl EpollEvent {
    fn events(&self) -> u32 {
        unsafe { core::ptr::addr_of!(self.events).read_unaligned() }
    }

    fn data(&self) -> u64 {
        unsafe { core::ptr::addr_of!(self.data).read_unaligned() }
    }

    fn zero_unaligned(&mut self) {
        unsafe {
            core::ptr::addr_of_mut!(self.events).write_unaligned(0);
            core::ptr::addr_of_mut!(self.data).write_unaligned(0);
        }
    }
}

fn fail(msg: &str) -> ! {
    println!("USERSPACE EPOLL: FAIL - {}", msg);
    process::exit(1);
}

fn syscall_i32(ret: u64, name: &str) -> i32 {
    let signed = ret as i64;
    if signed < 0 {
        println!("  {} returned errno {}", name, -signed);
        fail(name);
    }
    signed as i32
}

fn epoll_create1(flags: i32) -> i32 {
    let ret = unsafe { raw::syscall1(nr::EPOLL_CREATE1, flags as u64) };
    syscall_i32(ret, "epoll_create1")
}

fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: &mut EpollEvent) {
    let ret = unsafe {
        raw::syscall4(
            nr::EPOLL_CTL,
            epfd as u64,
            op as u64,
            fd as u64,
            event as *mut EpollEvent as u64,
        )
    };
    let _ = syscall_i32(ret, "epoll_ctl");
}

fn epoll_wait(epfd: i32, events: &mut [EpollEvent], timeout_ms: i32) -> i32 {
    let ret = unsafe {
        raw::syscall6(
            nr::EPOLL_PWAIT,
            epfd as u64,
            events.as_mut_ptr() as u64,
            events.len() as u64,
            timeout_ms as u64,
            0,
            0,
        )
    };
    syscall_i32(ret, "epoll_wait")
}

fn expect_ready(epfd: i32, expected_data: u64, label: &str) {
    let mut events = [EpollEvent::default(); 1];
    let count = epoll_wait(epfd, &mut events, 0);
    println!(
        "  {}: epoll_wait returned {}, events={:#x}, data={:#x}",
        label,
        count,
        events[0].events(),
        events[0].data()
    );

    if count != 1 {
        fail("epoll_wait should return one ready event");
    }
    if events[0].events() & EPOLLIN == 0 {
        fail("ready event should include EPOLLIN");
    }
    if events[0].data() != expected_data {
        fail("ready event data mismatch");
    }
}

fn create_ready_epoll(data: u64) -> (i32, libbreenix::types::Fd, libbreenix::types::Fd) {
    let (read_fd, write_fd) = io::pipe().unwrap_or_else(|_| fail("pipe failed"));
    let epfd = epoll_create1(0);

    let mut event = EpollEvent {
        events: EPOLLIN,
        data,
    };
    epoll_ctl(epfd, EPOLL_CTL_ADD, read_fd.raw() as i32, &mut event);

    let written = io::write(write_fd, b"x").unwrap_or_else(|_| fail("pipe write failed"));
    if written != 1 {
        fail("pipe write returned short count");
    }

    (epfd, read_fd, write_fd)
}

fn test_basic_ready_pipe() {
    println!("Phase 1: basic epoll ready pipe");
    let (epfd, read_fd, write_fd) = create_ready_epoll(0xE901_0001);
    expect_ready(epfd, 0xE901_0001, "basic");
    let _ = io::close(read_fd);
    let _ = io::close(write_fd);
    println!("  EPOLL_BASIC_READY_PASS");
}

fn test_fork_cow_wait_buffer() {
    println!("Phase 2: fork child epoll_wait into CoW stack buffer");
    let (epfd, read_fd, write_fd) = create_ready_epoll(0xC0FF_EE68);

    let mut child_events = [EpollEvent::default(); 1];
    child_events[0].zero_unaligned();

    match fork() {
        Ok(ForkResult::Child) => {
            let count = epoll_wait(epfd, &mut child_events, 0);
            println!(
                "[CHILD] epoll_wait returned {}, events={:#x}, data={:#x}",
                count,
                child_events[0].events(),
                child_events[0].data()
            );
            if count == 1
                && child_events[0].events() & EPOLLIN != 0
                && child_events[0].data() == 0xC0FF_EE68
            {
                println!("EPOLL_COW_WAIT_CHILD_PASS");
                process::exit(0);
            }
            println!("EPOLL_COW_WAIT_CHILD_FAIL");
            process::exit(1);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            let mut status = 0i32;
            match waitpid(child_pid.raw() as i32, &mut status, 0) {
                Ok(pid) if pid.raw() == child_pid.raw() => {}
                _ => fail("waitpid failed for epoll CoW child"),
            }
            if !wifexited(status) || wexitstatus(status) != 0 {
                fail("epoll CoW child failed");
            }
        }
        Err(_) => fail("fork failed for epoll CoW test"),
    }

    let _ = io::close(read_fd);
    let _ = io::close(write_fd);
    println!("  EPOLL_COW_WAIT_PASS");
}

fn main() {
    println!("=== Epoll Test Program ===");
    test_basic_ready_pipe();
    test_fork_cow_wait_buffer();
    println!("USERSPACE EPOLL: ALL TESTS PASSED");
    println!("EPOLL_TEST_PASSED");
    process::exit(0);
}
