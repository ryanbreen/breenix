//! Console adaptation ownership, queue publication, producer and oracle ratchets.
use std::fs;
fn read(path: &str) -> String {
    fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path)).unwrap()
}
fn compact(s: &str) -> String {
    s.split_whitespace().collect()
}
#[test]
fn same_guard_check_prepare_and_common_enqueue_wake() {
    let s = compact(&read("kernel/src/ipc/stdin.rs"));
    let adapter = s
        .split("pub(crate)fnread_or_prepare")
        .nth(1)
        .unwrap()
        .split("///Bootobserver")
        .next()
        .unwrap();
    assert!(adapter.contains("arch_without_interrupts(||{letmutbuffer=STDIN_BUFFER.lock();"));
    assert!(adapter.contains("returnOk(buffer.read_bytes(buf))"));
    assert!(adapter.contains("INPUT_READERS.prepare_to_wait_checked("));
    assert!(adapter.contains("||buffer.is_empty()"));
    assert!(!adapter.contains("drop(buffer)"));
    let push = s
        .split("fnpush_byte(&mutself")
        .nth(1)
        .unwrap()
        .split("fnread_bytes")
        .next()
        .unwrap();
    assert!(push.contains("self.len+=1;"));
    assert!(push.contains("INPUT_READERS.wake_up_deferred();"));
    assert!(push.find("self.len+=1;").unwrap() < push.find("wake_up_deferred()").unwrap());
    let helper = compact(&read("kernel/src/syscall/blocking_io.rs"));
    assert!(helper.contains("wait_prepared(&crate::ipc::stdin::INPUT_READERS,outcome)?"));
}
#[test]
fn device_modes_poll_and_non_sleeping_device_api() {
    let h = compact(&read("kernel/src/syscall/handlers.rs"));
    let arm = h
        .split("//Readfromdevfsdevice")
        .nth(1)
        .unwrap()
        .split("FdKind::DevfsDirectory")
        .next()
        .unwrap();
    assert!(arm.contains("fd_entry.status_flags&crate::ipc::fd::status_flags::O_NONBLOCK"));
    assert!(
        arm.find("drop(manager_guard)").unwrap() < arm.find("blocking_io::read_console").unwrap()
    );
    let f = compact(&read("kernel/src/syscall/fs.rs"));
    assert_eq!(
        f.matches("FdKind::Device(device.device_type),ifflags&O_CLOEXEC")
            .count(),
        2
    );
    let opens = f
        .split("fnhandle_devfs_open(")
        .nth(1)
        .unwrap()
        .split("fnhandle_devpts_open(")
        .next()
        .unwrap();
    assert_eq!(
        opens
            .matches("flags&crate::ipc::fd::status_flags::O_NONBLOCK")
            .count(),
        3
    );
    assert_eq!(
        opens.matches("fd_table.alloc_with_entry(fd_kind)").count(),
        2
    );
    let p = compact(&read("kernel/src/ipc/poll.rs"));
    assert!(p.contains("(events&events::POLLIN)!=0&&crate::ipc::stdin::has_data()"));
    let d = compact(&read("kernel/src/fs/devfs/mod.rs"));
    assert!(d.contains("crate::ipc::stdin::read_bytes(buf).map_err(|e|-e)"));
}
#[test]
fn observer_is_read_only_and_injection_requires_witness() {
    let s = compact(&read("kernel/src/syscall/blocking_io_oracle.rs"));
    assert!(s.contains("queued&&blocked&&in_syscall&&occupancy==0"));
    assert!(s.contains("if!crate::ipc::stdin::push_byte_from_irq(witness.identityasu8)"));
    assert!(!s.contains("stdin::read_bytes"));
    assert!(!s.contains("take_waiter"));
    assert!(s.contains("FdKind::Device(DeviceType::Tty)"));
}
#[test]
fn both_architectures_score_all_console_arms() {
    let driver = read("userspace/programs/src/console_read_oracle.rs");
    let gate = read("docker/qemu/run-blocking-io-oracle-gate.sh");
    let score = read("scripts/score-blocking-io-oracle.py");
    for arm in [
        "blocking",
        "nonblock_open",
        "nonblock_fcntl",
        "readiness_partial",
        "eintr",
        "immediate",
    ] {
        assert!(driver.contains(&format!("\"{arm}\"")));
        assert!(gate
            .split("CONSOLE_ARMS=(")
            .nth(1)
            .unwrap()
            .split(')')
            .next()
            .unwrap()
            .split_whitespace()
            .any(|a| a == arm));
        assert!(score.contains(&format!("'{arm}'")));
    }
    assert!(driver.contains("parked(fd, tid)?"));
    assert!(driver.contains("reap(pid)"));
    assert!(driver.contains("clean(fd, tid)?"));
    assert!(driver.contains("bytes[..2] == *b\"89\""));
    assert!(score.contains("missing console record"));
}
#[test]
fn data_signal_and_immediate_controls_remain_observable() {
    let stdin = compact(&read("kernel/src/ipc/stdin.rs"));
    assert!(stdin.contains("buf[read]=self.buffer[self.read_pos]"));
    assert!(stdin.contains("letto_read=buf.len().min(self.len)"));
    let helper = compact(&read("kernel/src/syscall/blocking_io.rs"));
    assert!(helper.contains("check_signals_for_eintr().is_some()"));
    assert!(helper.contains("queue.take_waiter(tid);queue.finish_wait_for(tid);"));
    let handlers = compact(&read("kernel/src/syscall/handlers.rs"));
    let device = handlers
        .split("//Readfromdevfsdevice")
        .nth(1)
        .unwrap()
        .split("FdKind::DevfsDirectory")
        .next()
        .unwrap();
    assert!(device.contains("copy_to_user(buf_ptr,user_buf.as_ptr()asu64,n)"));
    assert!(handlers.contains("ifbuf_ptr==0||count==0"));
    let devfs = compact(&read("kernel/src/fs/devfs/mod.rs"));
    let read_arm = devfs
        .split("pubfndevice_read")
        .nth(1)
        .unwrap()
        .split("pubfndevice_write")
        .next()
        .unwrap();
    assert!(read_arm.contains("Ok(0)"));
    assert!(read_arm.contains("*byte=0;"));
    assert!(read_arm.contains("Ok(buf.len())"));
    let driver = compact(&read("userspace/programs/src/console_read_oracle.rs"));
    assert!(driver.contains("inject(fd,tid,b'8')?"));
    assert!(driver.contains("clean(fd,tid)?"));
    assert!(driver.contains("ifname==\"nonblock_fcntl\"{mode(fd,true)?;}"));
    assert!(driver.contains("errno(io::read(fd,&mut[0;1]))==-11"));
}
