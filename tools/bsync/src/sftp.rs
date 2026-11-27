//! The SFTP v3 operations needed to stream and acknowledge a staged upload.
//! https://datatracker.ietf.org/doc/html/draft-ietf-secsh-filexfer-02

use crate::Result;
use std::io::{Read, Write};

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const WRITE: u8 = 6;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const CREATE_EXCLUSIVE: u32 = 0x02 | 0x08 | 0x20;
const MAX_REPLY: usize = 256 * 1024;

pub fn send(input: &mut impl Write, output: &mut impl Read, mut file: impl Read) -> Result<()> {
    let mut sftp = Client {
        input,
        output,
        id: 0,
    };
    sftp.packet(INIT, &3u32.to_be_bytes())?;
    let version = sftp.reply()?;
    if !version.starts_with(&[VERSION, 0, 0, 0, 3]) {
        return Err("expected SFTP version 3".into());
    }

    let handle = sftp.create(b"payload")?;
    let mut offset = 0u64;
    let mut bytes = [0; 32 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        let mut fields = Vec::new();
        string(&mut fields, &handle);
        fields.extend(offset.to_be_bytes());
        string(&mut fields, &bytes[..count]);
        sftp.request_ok(WRITE, &fields)?;
        offset += count as u64;
    }
    sftp.close(&handle)?;
    // This marker tells the shell that the client received every write and
    // close acknowledgement. bput independently checks the staged bytes.
    let ready = sftp.create(b"ready")?;
    sftp.close(&ready)
}

struct Client<'a, W, R> {
    input: &'a mut W,
    output: &'a mut R,
    id: u32,
}

impl<W: Write, R: Read> Client<'_, W, R> {
    fn packet(&mut self, kind: u8, fields: &[u8]) -> Result<()> {
        let mut frame = Vec::with_capacity(fields.len() + 5);
        frame.extend(u32::try_from(fields.len() + 1)?.to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(fields);
        self.input.write_all(&frame)?;
        self.input.flush()?;
        Ok(())
    }

    fn reply(&mut self) -> Result<Vec<u8>> {
        let mut len = [0; 4];
        self.output.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if !(1..=MAX_REPLY).contains(&len) {
            return Err("invalid SFTP packet length".into());
        }
        let mut frame = vec![0; len];
        self.output.read_exact(&mut frame)?;
        Ok(frame)
    }

    fn request(&mut self, kind: u8, fields: &[u8]) -> Result<Vec<u8>> {
        self.id = self.id.wrapping_add(1);
        let id = self.id.to_be_bytes();
        let mut body = Vec::from(id);
        body.extend_from_slice(fields);
        self.packet(kind, &body)?;
        let reply = self.reply()?;
        if reply.get(1..5) != Some(&id) {
            return Err("unexpected SFTP reply ID".into());
        }
        Ok(reply)
    }

    fn create(&mut self, name: &[u8]) -> Result<Vec<u8>> {
        let mut fields = Vec::new();
        string(&mut fields, name);
        fields.extend(CREATE_EXCLUSIVE.to_be_bytes());
        fields.extend(0u32.to_be_bytes()); // Empty attributes, under remote umask 077.
        let reply = self.request(OPEN, &fields)?;
        if reply[0] == HANDLE {
            let len = number(reply.get(5..9))? as usize;
            if len > 0 && reply.len() == 9 + len {
                return Ok(reply[9..].to_vec());
            }
        }
        Err(reply_error(OPEN, &reply).into())
    }

    fn close(&mut self, handle: &[u8]) -> Result<()> {
        let mut fields = Vec::new();
        string(&mut fields, handle);
        self.request_ok(CLOSE, &fields)
    }

    fn request_ok(&mut self, kind: u8, fields: &[u8]) -> Result<()> {
        let reply = self.request(kind, fields)?;
        if reply[0] == STATUS && number(reply.get(5..9))? == 0 {
            return Ok(());
        }
        Err(reply_error(kind, &reply).into())
    }
}

fn number(field: Option<&[u8]>) -> Result<u32> {
    Ok(u32::from_be_bytes(
        field.ok_or("short SFTP reply")?.try_into()?,
    ))
}

fn reply_error(request: u8, reply: &[u8]) -> String {
    if reply[0] == STATUS {
        if let Ok(code) = number(reply.get(5..9)) {
            return format!("SFTP request {request} failed with status {code}");
        }
    }
    format!("invalid SFTP response to request {request}")
}

fn string(fields: &mut Vec<u8>, bytes: &[u8]) {
    fields.extend((bytes.len() as u32).to_be_bytes());
    fields.extend_from_slice(bytes);
}
