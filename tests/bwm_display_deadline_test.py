"""A request arriving near the read deadline gets its own switch/reply window."""
from pathlib import Path
import subprocess
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
class Deadline(unittest.TestCase):
    def test_switch_deadline_starts_with_request_execution(self):
        harness=r'''
extern crate self as libbreenix;
pub mod io {
    pub struct PollFd {pub fd:i32,pub events:i16,pub revents:i16}
    pub mod poll_events {pub const POLLIN:i16=1;pub const POLLOUT:i16=4;pub const POLLERR:i16=8;pub const POLLHUP:i16=16;pub const POLLNVAL:i16=32;}
}
pub mod ui_clock {
    pub fn now_ns()->u64 {4_900_000_000}
}
#[path="PROTOCOL"] mod display_protocol;
mod display_socket {
SOURCE
#[test] fn late_request_gets_reply() {
    let path=std::env::temp_dir().join(format!("bwm-deadline-{}",std::process::id()));
    let listener=UnixListener::bind(&path).unwrap();
    let (mut peer,stream)=UnixStream::pair().unwrap();
    peer.set_nonblocking(true).unwrap(); stream.set_nonblocking(true).unwrap();
    let fd=stream.as_raw_fd();
    let mut server=Server {listener,clients:vec![Client {stream,input:Vec::new(),output:Vec::new(),written:0,deadline:TIMEOUT}]};
    assert!(server.deadline().is_some());
    let mut poll=Vec::new(); server.append_poll(&mut poll); assert!(!poll.is_empty());
    peer.write_all(b"mode 1024x768\n").unwrap();
    server.service(&[PollFd{fd,events:poll_events::POLLIN,revents:poll_events::POLLIN}],4_900_000_000,|_| Err("mode refused".into()));
    // A two-second mode operation has completed; its response must still be sent.
    server.service(&[PollFd{fd,events:poll_events::POLLOUT,revents:poll_events::POLLOUT}],6_900_000_000,|_| panic!("must execute only once"));
    let mut response=String::new(); peer.read_to_string(&mut response).unwrap();
    assert!(response.contains("mode refused"),"lost response: {response:?}");
    drop(server); std::fs::remove_file(path).unwrap();
}
}
'''.replace('PROTOCOL',str(ROOT/'userspace/programs/src/bwm/display_protocol.rs')).replace('SOURCE',(ROOT/'userspace/programs/src/bwm/display_socket.rs').read_text().replace('//!','//'))
        with tempfile.TemporaryDirectory() as directory:
            p=Path(directory);(p/'test.rs').write_text(harness)
            for cmd in [['taskpolicy','-b','rustc','--edition=2021','--test',str(p/'test.rs'),'-o',str(p/'test')],[str(p/'test')]]:
                r=subprocess.run(cmd,capture_output=True,text=True)
                self.assertEqual(r.returncode,0,r.stdout+r.stderr)
if __name__=='__main__': unittest.main()
