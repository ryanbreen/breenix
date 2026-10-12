//! Scratch fixture: print one failing userspace line and then sleep forever.

fn main() {
    println!("SUITE smoke START cases=5");
    println!("SIGKILL_TEARDOWN_TEST: FAIL");
    loop {
        let _ = libbreenix::time::sleep_ms(1000);
    }
}
