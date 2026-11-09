use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::thread;

#[path = "../../../userspace/programs/src/bwm/clipboard.rs"]
mod clipboard;

#[test]
fn set_and_get_over_socketpair() {
    let (mut client, mut server) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || {
        let mut store = clipboard::Store::new();
        for _ in 0..2 {
            let mut decoder = clipboard::Decoder::new();
            loop {
                let mut byte = [0];
                server.read_exact(&mut byte).unwrap();
                if let Some(request) = decoder.feed(&byte).1 {
                    let request = request.unwrap();
                    assert_eq!(request.window, 42);
                    server.write_all(&store.execute(request).encode()).unwrap();
                    break;
                }
            }
        }
    });
    libclipboard::set_on(&mut client, 42, b"echo pasted").unwrap();
    assert_eq!(libclipboard::get_on(&mut client, 42).unwrap(), b"echo pasted");
    worker.join().unwrap();
}
