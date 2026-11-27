//! A single-threaded SFTP server model for `send`. Replies become readable
//! as soon as their request is written, unless the model holds WRITE
//! replies back until a full window of writes has arrived and then answers
//! them newest first. The model counts how many requests the client has
//! outstanding: written, with the reply not yet read. A client that waits
//! for a reply that is not available gets an error instead of a hang.

use super::{send, WINDOW};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::rc::Rc;

const CHUNK: usize = 32 * 1024;

#[derive(Default)]
struct Model {
    incoming: Vec<u8>,
    replies: VecDeque<Vec<u8>>,
    reply_offset: usize,
    outstanding: usize,
    max_outstanding: usize,
    writes: Vec<(Vec<u8>, u64, Vec<u8>)>,
    files: Vec<(Vec<u8>, bool)>,
    fail_write: Option<usize>,
    /// Hold WRITE replies until WINDOW are pending, then release them in
    /// reverse order.
    hold_writes: bool,
    held: Vec<Vec<u8>>,
}

fn field(bytes: &[u8], pos: &mut usize) -> Vec<u8> {
    let len = u32::from_be_bytes(bytes[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    let value = bytes[*pos..*pos + len].to_vec();
    *pos += len;
    value
}

fn status(id: &[u8], code: u32) -> Vec<u8> {
    let mut body = vec![101];
    body.extend_from_slice(id);
    body.extend(code.to_be_bytes());
    body.extend(0u32.to_be_bytes());
    body.extend(0u32.to_be_bytes());
    body
}

impl Model {
    fn handle(&mut self, frame: &[u8]) {
        let write = frame[0] == 6;
        let reply = match frame[0] {
            1 => vec![2, 0, 0, 0, 3],
            3 => {
                let id = &frame[1..5];
                let mut pos = 5;
                let name = field(frame, &mut pos);
                self.files.push((name, false));
                let handle = format!("h{}", self.files.len()).into_bytes();
                let mut body = vec![102];
                body.extend_from_slice(id);
                body.extend((handle.len() as u32).to_be_bytes());
                body.extend(handle);
                body
            }
            6 => {
                let id = &frame[1..5];
                let mut pos = 5;
                let handle = field(frame, &mut pos);
                let offset = u64::from_be_bytes(frame[pos..pos + 8].try_into().unwrap());
                pos += 8;
                let data = field(frame, &mut pos);
                self.writes.push((handle, offset, data));
                let failed = self.fail_write == Some(self.writes.len());
                status(id, u32::from(failed))
            }
            4 => {
                let id = &frame[1..5];
                let mut pos = 5;
                let handle = field(frame, &mut pos);
                let index: usize = std::str::from_utf8(&handle[1..]).unwrap().parse().unwrap();
                self.files[index - 1].1 = true;
                status(id, 0)
            }
            kind => panic!("unexpected SFTP request {kind}"),
        };
        let mut wire = (reply.len() as u32).to_be_bytes().to_vec();
        wire.extend(reply);
        if write && self.hold_writes {
            self.held.push(wire);
            if self.held.len() == WINDOW {
                while let Some(wire) = self.held.pop() {
                    self.replies.push_back(wire);
                }
            }
        } else {
            self.replies.push_back(wire);
        }
        self.outstanding += 1;
        self.max_outstanding = self.max_outstanding.max(self.outstanding);
    }
}

struct Input(Rc<RefCell<Model>>);
struct Output(Rc<RefCell<Model>>);

impl Write for Input {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut model = self.0.borrow_mut();
        model.incoming.extend_from_slice(data);
        while model.incoming.len() >= 4 {
            let len = u32::from_be_bytes(model.incoming[..4].try_into().unwrap()) as usize;
            if model.incoming.len() < 4 + len {
                break;
            }
            let frame: Vec<u8> = model.incoming.drain(..4 + len).skip(4).collect();
            model.handle(&frame);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Output {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut model = self.0.borrow_mut();
        let offset = model.reply_offset;
        let Some(front) = model.replies.front() else {
            return Err(io::Error::other("client waited for a reply it did not request"));
        };
        let n = out.len().min(front.len() - offset);
        out[..n].copy_from_slice(&front[offset..offset + n]);
        if offset + n == front.len() {
            model.replies.pop_front();
            model.reply_offset = 0;
            model.outstanding -= 1;
        } else {
            model.reply_offset += n;
        }
        Ok(n)
    }
}

fn upload(file: &[u8], fail_write: Option<usize>) -> (Model, Result<(), String>) {
    upload_to(Model { fail_write, ..Model::default() }, file)
}

fn upload_to(model: Model, file: &[u8]) -> (Model, Result<(), String>) {
    let model = Rc::new(RefCell::new(model));
    let result = send(&mut Input(model.clone()), &mut Output(model.clone()), file)
        .map_err(|error| error.to_string());
    let model = Rc::try_unwrap(model).ok().unwrap().into_inner();
    (model, result)
}

#[test]
fn writes_are_pipelined_up_to_the_window_and_cover_the_file_once() {
    let file: Vec<u8> = (0..(3 * WINDOW * CHUNK + 100)).map(|i| (i % 251) as u8).collect();
    let (model, result) = upload(&file, None);
    result.unwrap();

    assert!(WINDOW > 1);
    assert_eq!(model.max_outstanding, WINDOW, "a full window of writes in flight, and no more");
    let mut expected = 0u64;
    let mut written = Vec::new();
    for (handle, offset, data) in &model.writes {
        assert_eq!(handle, b"h1");
        assert_eq!(*offset, expected, "each byte is written once, in order");
        assert!(data.len() <= CHUNK);
        expected += data.len() as u64;
        written.extend_from_slice(data);
    }
    assert_eq!(written, file);
    // The payload is closed, and only then is the ready marker made and closed.
    assert_eq!(model.files, [(b"payload".to_vec(), true), (b"ready".to_vec(), true)]);
    assert!(model.replies.is_empty(), "every reply was read");
}

#[test]
fn a_failed_pipelined_write_fails_the_upload_without_marking_it_ready() {
    // The server answers the first window's writes only once all of them
    // have arrived, newest first, and the third one failed. The upload must
    // report that status, which needs the window of writes in flight and
    // each reply matched to its request, and must not mark itself ready.
    let file = vec![7u8; 2 * WINDOW * CHUNK];
    let model = Model { fail_write: Some(3), hold_writes: true, ..Model::default() };
    let (model, result) = upload_to(model, &file);
    let error = result.unwrap_err();
    assert!(error.contains("SFTP request 6 failed with status 1"), "{error}");
    assert!(model.files.iter().all(|(name, _)| name != b"ready"));
}
