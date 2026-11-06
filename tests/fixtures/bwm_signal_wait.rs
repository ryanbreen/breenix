// The notification transport is a host socket pair; the handler is production code.
mod signal_wake {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    pub static WRITE: Mutex<Option<UnixStream>> = Mutex::new(None);
    pub fn notify() {
        if let Some(stream) = WRITE.lock().unwrap().as_mut() {
            stream.write_all(&[1]).unwrap();
        }
    }
}
@HANDLER@

#[test]
fn notification_survives_the_unblock_to_poll_gap() {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    let (mut read, write) = UnixStream::pair().unwrap();
    read.set_nonblocking(true).unwrap();
    *signal_wake::WRITE.lock().unwrap() = Some(write);
    // Force the signal to finish before the wait begins. A no-op handler loses it.
    child_exited(17);
    let mut byte = [0];
    assert_eq!(read.read(&mut byte).ok(), Some(1), "child exit was lost before poll");
    // Keep the host transport callable even when testing the old no-op handler.
    signal_wake::notify();
    assert_eq!(read.read(&mut byte).ok(), Some(1));
}
