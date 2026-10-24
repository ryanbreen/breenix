//! #2411: race single-write console records against real signal deaths.
//! The raw UART capture, checked by check-serial-records.py, is the oracle.
use libbreenix::{fs, io, memory, process, signal};
use libbreenix::process::ForkResult;
use libbreenix::types::Fd;
use std::sync::atomic::{AtomicU32, Ordering};

const RECORDS: usize = 96;
const DEATHS: usize = 128;
static CONSOLE: AtomicU32 = AtomicU32::new(1);

fn write(bytes: &[u8]) {
    // Require one complete write, including for the 4096-byte records (#2411).
    let fd = Fd::from_raw(CONSOLE.load(Ordering::Relaxed) as u64);
    assert_eq!(io::write(fd, bytes).unwrap(), bytes.len());
}

fn wait(pid: i32, expected: i32) {
    let mut status = 0;
    assert_eq!(process::waitpid(pid, &mut status, 0).unwrap().raw(), pid as u64);
    assert_eq!(status, expected);
}

fn main() {
    // The program launcher captures stdout through a pipe. Bypass that pipe so
    // the syscall being tested is one write to the actual console device.
    let console = fs::open("/dev/console\0", fs::O_WRONLY).unwrap();
    CONSOLE.store(console.raw() as u32, Ordering::Relaxed);
    let shared = memory::mmap(std::ptr::null_mut(), 4096,
        memory::PROT_READ | memory::PROT_WRITE,
        memory::MAP_SHARED | memory::MAP_ANONYMOUS, -1, 0).unwrap();
    let ready = unsafe { &*shared.cast::<AtomicU32>() };
    let go = unsafe { &*shared.add(8).cast::<AtomicU32>() };
    write(b"SERIAL_RACE BEGIN writers=2 records=96 deaths=128\n");
    let mut writers = Vec::new();
    for writer in 0..2 {
        match process::fork().unwrap() {
            ForkResult::Child => {
                ready.fetch_add(1, Ordering::SeqCst);
                while go.load(Ordering::SeqCst) == 0 { process::yield_now().unwrap(); }
                for seq in 0..RECORDS {
                    let size = [80, 1024, 4096][seq % 3];
                    let header = format!("SERIAL_RECORD writer={writer} seq={seq:03} ");
                    let mut record = vec![b'A' + writer; size];
                    record[..header.len()].copy_from_slice(header.as_bytes());
                    record[size - 1] = b'\n';
                    write(&record);
                    process::yield_now().unwrap();
                }
                process::exit(0);
            }
            ForkResult::Parent(pid) => writers.push(pid.raw() as i32),
        }
    }
    while ready.load(Ordering::SeqCst) != 2 { process::yield_now().unwrap(); }
    go.store(1, Ordering::SeqCst);
    let mut victims = Vec::new();
    for _ in 0..DEATHS {
        match process::fork().unwrap() {
            ForkResult::Child => {
                signal::kill(process::getpid().unwrap().raw() as i32, 15).unwrap();
                process::exit(1);
            }
            ForkResult::Parent(pid) => {
                let pid = pid.raw() as i32;
                wait(pid, 15);
                victims.push(pid);
            }
        }
    }
    for pid in writers { wait(pid, 0); }
    for pid in victims { write(format!("SERIAL_VICTIM pid={pid}\n").as_bytes()); }
    write(b"SERIAL_RACE DONE\n");
    memory::munmap(shared, 4096).unwrap();
}
