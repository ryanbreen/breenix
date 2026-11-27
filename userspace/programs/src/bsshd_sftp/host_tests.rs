// Run with: rustc --edition=2021 --test userspace/programs/src/bsshd_sftp/host_tests.rs
#[path = "../bsshd_sftp.rs"]
mod sftp;

use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{self as unix, MetadataExt};
use std::path::{Path, PathBuf};

const OK: u32 = 0;
const EOF: u32 = 1;
const NO_SUCH_FILE: u32 = 2;
const PERMISSION_DENIED: u32 = 3;
const FAILURE: u32 = 4;
const BAD_MESSAGE: u32 = 5;
const OP_UNSUPPORTED: u32 = 8;
const MAX_BODY: usize = 256 * 1024;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bsshd-sftp-{}-{name}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Default)]
struct Wire {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Wire {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let fragment = data.len().min(97);
        self.input.read(&mut data[..fragment])
    }
}

impl Write for Wire {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.output.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn u32_field(value: u32) -> [u8; 4] {
    value.to_be_bytes()
}

fn string(value: &[u8]) -> Vec<u8> {
    [&u32_field(value.len() as u32), value].concat()
}

fn path(value: &Path) -> Vec<u8> {
    string(value.as_os_str().as_bytes())
}

fn frame(kind: u8, fields: &[&[u8]]) -> Vec<u8> {
    let body = [&[kind][..], &fields.concat()].concat();
    [&u32_field(body.len() as u32), body.as_slice()].concat()
}

fn request(kind: u8, id: u32, fields: &[&[u8]]) -> Vec<u8> {
    frame(kind, &[&u32_field(id), &fields.concat()])
}

fn replies(input: Vec<u8>, target_first: bool) -> (io::Result<()>, Vec<Vec<u8>>) {
    let mut wire = Wire {
        input: Cursor::new(input),
        ..Wire::default()
    };
    let result = sftp::serve(&mut wire, target_first);
    let mut frames = Vec::new();
    let mut output = wire.output.as_slice();
    while !output.is_empty() {
        let len = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
        assert!((1..=MAX_BODY).contains(&len));
        frames.push(output[4..4 + len].to_vec());
        output = &output[4 + len..];
    }
    (result, frames)
}

fn session(requests: &[Vec<u8>], target_first: bool) -> Vec<Vec<u8>> {
    let init = frame(1, &[&u32_field(3)]);
    let (result, mut output) = replies([init, requests.concat()].concat(), target_first);
    result.unwrap();
    assert_eq!(output.remove(0), [2, 0, 0, 0, 3]);
    assert_eq!(output.len(), requests.len());
    for (sent, received) in requests.iter().zip(&output) {
        assert_eq!(received[1..5], sent[5..9]);
    }
    output
}

fn reply_payload(response: &[u8], kind: u8) -> &[u8] {
    assert_eq!(response[0], kind, "unexpected response: {response:?}");
    &response[5..]
}

fn take_string<'a>(bytes: &mut &'a [u8]) -> &'a [u8] {
    let len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    let (value, tail) = bytes[4..].split_at(len);
    *bytes = tail;
    value
}

fn status(response: &[u8], code: u32) {
    let body = reply_payload(response, 101);
    assert_eq!(body[..4], u32_field(code), "{response:?}");
    let mut strings = &body[4..];
    assert!(std::str::from_utf8(take_string(&mut strings)).is_ok());
    take_string(&mut strings);
    assert!(strings.is_empty());
}

fn name(response: &[u8]) -> (&[u8], &[u8], &[u8]) {
    let body = reply_payload(response, 104);
    assert_eq!(body[..4], u32_field(1));
    let mut fields = &body[4..];
    (take_string(&mut fields), take_string(&mut fields), fields)
}

fn handle(number: u64) -> Vec<u8> {
    string(&number.to_be_bytes())
}

fn attributes(response: &[u8], size: u64, mode: u32, times: (u32, u32)) {
    let body = reply_payload(response, 105);
    assert_eq!(body.len(), 32);
    assert_eq!(body[..4], u32_field(15));
    assert_eq!(body[4..12], size.to_be_bytes());
    assert_eq!(body[20..24], u32_field(mode));
    assert_eq!(body[24..28], u32_field(times.0));
    assert_eq!(body[28..32], u32_field(times.1));
}

#[test]
fn negotiates_v3_and_bounds_packet_lengths() {
    let init = frame(1, &[&u32_field(7), &string(b"vendor"), &string(b"data")]);
    let (result, output) = replies(init, false);
    result.unwrap();
    assert_eq!(output, [vec![2, 0, 0, 0, 3]]);

    let extension = string(b"test");
    let padding = vec![0; MAX_BODY - 5 - extension.len()];
    let output = session(&[request(200, 1, &[&extension, &padding])], false);
    status(&output[0], OP_UNSUPPORTED);

    let init = frame(1, &[&u32_field(3)]);
    for malformed in [
        u32_field(0).to_vec(),
        u32_field(MAX_BODY as u32 + 1).to_vec(),
        u32_field(u32::MAX).to_vec(),
        vec![0, 0, 0],
        vec![0, 0, 0, 5, 3, 0],
        frame(3, &[]),
        frame(1, &[&u32_field(3)]),
    ] {
        let (result, output) = replies([init.clone(), malformed].concat(), false);
        assert!(result.is_err());
        assert_eq!(output, [vec![2, 0, 0, 0, 3]]);
    }

    for malformed_init in [
        request(17, 9, &[&string(b".")]),
        frame(1, &[&[0, 0, 0]]),
        frame(1, &[&u32_field(3), &string(b"missing extension data")]),
        frame(1, &[&u32_field(2)]),
    ] {
        let (result, output) = replies(malformed_init, false);
        assert!(result.is_err());
        assert!(output.is_empty());
    }
}

#[test]
fn answers_malformed_requests_with_id_and_continues() {
    let bad_path = u32_field(u32::MAX);
    let huge_attr_count = [&u32_field(0x8000_0000)[..], &u32_field(u32::MAX)].concat();
    let huge_handle = string(&[0; 257]);
    let requests = [
        request(3, 1, &[&bad_path]),
        request(9, 2, &[&string(b"."), &huge_attr_count]),
        request(9, 3, &[&string(b"."), &u32_field(16)]),
        request(17, 4, &[&string(b"a\0b")]),
        request(17, 5, &[&string(b"."), &[0]]),
        request(4, 6, &[&huge_handle]),
        request(200, 7, &[&bad_path]),
        request(3, 8, &[&string(b"."), &u32_field(0x40), &u32_field(0)]),
        request(201, 9, &[]),
        request(16, 10, &[&string(b"")]),
    ];
    let output = session(&requests, false);
    for response in &output[..9] {
        status(response, BAD_MESSAGE);
    }
    let (resolved, _, attrs) = name(&output[9]);
    assert_eq!(
        resolved,
        std::env::current_dir().unwrap().as_os_str().as_bytes()
    );
    assert_eq!(attrs, u32_field(0));
}

#[test]
fn transfers_data_and_updates_attributes_by_handle_and_path() {
    let scratch = Scratch::new("transfer");
    let file = path(&scratch.path("file"));
    let first = handle(1);
    let second = handle(2);
    let third = handle(3);
    let initial = [&u32_field(4)[..], &u32_field(0o600)].concat();
    let update = [
        &u32_field(13)[..],
        &5u64.to_be_bytes(),
        &u32_field(0o600),
        &u32_field(1_700_000_001),
        &u32_field(1_700_000_002),
    ]
    .concat();
    let path_update = [
        &u32_field(12)[..],
        &u32_field(0o640),
        &u32_field(3),
        &u32_field(4),
    ]
    .concat();
    let requests = [
        request(3, 1, &[&file, &u32_field(0x3b), &initial]),
        request(6, 2, &[&first, &2u64.to_be_bytes(), &string(b"abcdef")]),
        request(5, 3, &[&first, &2u64.to_be_bytes(), &u32_field(3)]),
        request(5, 4, &[&first, &8u64.to_be_bytes(), &u32_field(1)]),
        request(10, 5, &[&first, &update]),
        request(8, 6, &[&first]),
        request(4, 7, &[&first]),
        request(4, 8, &[&first]),
        request(3, 9, &[&file, &u32_field(0x06), &u32_field(0)]),
        request(6, 10, &[&second, &0u64.to_be_bytes(), &string(b"XY")]),
        request(4, 11, &[&second]),
        request(3, 12, &[&file, &u32_field(1), &u32_field(0)]),
        request(5, 13, &[&third, &0u64.to_be_bytes(), &u32_field(u32::MAX)]),
        request(4, 14, &[&third]),
        request(9, 15, &[&file, &path_update]),
        request(17, 16, &[&file]),
        request(7, 17, &[&path(&scratch.path("missing"))]),
    ];
    let output = session(&requests, false);
    assert_eq!(reply_payload(&output[0], 102), first);
    status(&output[1], OK);
    assert_eq!(reply_payload(&output[2], 103), string(b"abc"));
    status(&output[3], EOF);
    status(&output[4], OK);
    attributes(&output[5], 5, 0o100600, (1_700_000_001, 1_700_000_002));
    status(&output[6], OK);
    status(&output[7], FAILURE);
    assert_eq!(reply_payload(&output[8], 102), second);
    status(&output[9], OK);
    status(&output[10], OK);
    assert_eq!(reply_payload(&output[11], 102), third);
    assert_eq!(reply_payload(&output[12], 103), string(b"\0\0abcXY"));
    status(&output[13], OK);
    status(&output[14], OK);
    attributes(&output[15], 7, 0o100640, (3, 4));
    status(&output[16], NO_SUCH_FILE);
    assert_eq!(fs::read(scratch.path("file")).unwrap(), b"\0\0abcXY");
}

#[test]
fn lists_renames_and_removes_without_overwriting() {
    let scratch = Scratch::new("namespace");
    let directory = path(&scratch.path("dir"));
    let original = path(&scratch.path("original"));
    let moved = path(&scratch.path("dir/\u{e9}"));
    let occupied = path(&scratch.path("occupied"));
    fs::write(scratch.path("original"), b"source").unwrap();
    fs::write(scratch.path("occupied"), b"destination").unwrap();

    let requests = [
        request(14, 1, &[&directory, &u32_field(0)]),
        request(18, 2, &[&original, &occupied]),
        request(18, 3, &[&original, &moved]),
        request(11, 4, &[&directory]),
        request(12, 5, &[&handle(1)]),
        request(12, 6, &[&handle(1)]),
        request(4, 7, &[&handle(1)]),
        request(13, 8, &[&moved]),
        request(18, 9, &[&directory, &original]),
        request(15, 10, &[&original]),
    ];
    let output = session(&requests, false);
    status(&output[0], OK);
    status(&output[1], FAILURE);
    status(&output[2], OK);
    assert_eq!(reply_payload(&output[3], 102), handle(1));
    let (short, long, attrs) = name(&output[4]);
    assert_eq!(short, "é".as_bytes());
    assert!(long.ends_with(short));
    assert_eq!(attrs[4..12], 6u64.to_be_bytes());
    status(&output[5], EOF);
    for response in &output[6..] {
        status(response, OK);
    }
    assert_eq!(fs::read(scratch.path("occupied")).unwrap(), b"destination");
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
}

#[test]
fn resolves_symlinks_in_both_wire_orders_and_preserves_path_bytes() {
    let scratch = Scratch::new("symlink");
    let filename = std::ffi::OsStr::new("file-é");
    let file = scratch.0.join(filename);
    fs::write(&file, b"data").unwrap();
    let link = path(&scratch.path("link"));
    let target = string(filename.as_bytes());

    let requests = [
        request(20, 1, &[&link, &target]),
        request(19, 2, &[&link]),
        request(7, 3, &[&link]),
        request(17, 4, &[&link]),
        request(16, 5, &[&link]),
    ];
    let output = session(&requests, false);
    status(&output[0], OK);
    assert_eq!(name(&output[1]).0, filename.as_bytes());
    let mode =
        |reply: &[u8]| u32::from_be_bytes(reply_payload(reply, 105)[20..24].try_into().unwrap());
    assert_eq!(mode(&output[2]) & 0o170000, 0o120000);
    assert_eq!(mode(&output[3]) & 0o170000, 0o100000);
    assert_eq!(
        name(&output[4]).0,
        fs::canonicalize(file).unwrap().as_os_str().as_bytes()
    );

    let other = path(&scratch.path("other"));
    let raw_target = string(b"file-\xff");
    let output = session(
        &[
            request(20, 6, &[&raw_target, &other]),
            request(19, 7, &[&other]),
        ],
        true,
    );
    status(&output[0], OK);
    assert_eq!(name(&output[1]).0, b"file-\xff");
}

#[test]
fn kernel_denies_write_access_to_another_users_file_through_symlink() {
    let scratch = Scratch::new("permissions");
    // A system-owned, normally world-readable file. OPEN uses neither CREATE
    // nor TRUNCATE and sends no WRITE request, even if the assertion fails.
    let other_users_file = Path::new("/etc/hosts");
    assert_ne!(
        fs::metadata(other_users_file).unwrap().uid(),
        fs::metadata(&scratch.0).unwrap().uid()
    );
    let link = scratch.path("protected");
    unix::symlink(other_users_file, &link).unwrap();
    let output = session(
        &[request(3, 1, &[&path(&link), &u32_field(2), &u32_field(0)])],
        false,
    );
    status(&output[0], PERMISSION_DENIED);
}
