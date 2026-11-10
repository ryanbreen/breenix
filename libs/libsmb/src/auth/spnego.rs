//! DER SPNEGO tokens offering only NTLMSSP (RFC 4178).
use crate::Error;
const SPNEGO: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x02];
const NTLM: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x02, 0x0a];
fn tlv(tag: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if data.len() < 128 {
        out.push(data.len() as u8);
    } else {
        let bytes = data.len().to_be_bytes();
        let first = bytes
            .iter()
            .position(|b| *b != 0)
            .unwrap_or(bytes.len() - 1);
        out.push(0x80 | (bytes.len() - first) as u8);
        out.extend_from_slice(&bytes[first..]);
    }
    out.extend_from_slice(data);
    out
}
pub fn encode_init(token: &[u8]) -> Vec<u8> {
    let mut fields = tlv(0xa0, &tlv(0x30, &tlv(0x06, NTLM)));
    fields.extend_from_slice(&tlv(0xa2, &tlv(0x04, token)));
    let mut body = tlv(0x06, SPNEGO);
    body.extend_from_slice(&tlv(0xa0, &tlv(0x30, &fields)));
    tlv(0x60, &body)
}
pub fn encode_response(token: &[u8]) -> Vec<u8> {
    tlv(0xa1, &tlv(0x30, &tlv(0xa2, &tlv(0x04, token))))
}

fn take<'a>(input: &mut &'a [u8], tag: u8) -> Result<&'a [u8], Error> {
    if input.len() < 2 || input[0] != tag {
        return Err(Error);
    }
    let n = input[1];
    let (len, head) = if n & 0x80 == 0 {
        (n as usize, 2)
    } else {
        let count = (n & 0x7f) as usize;
        if count == 0
            || count > core::mem::size_of::<usize>()
            || input.len() < 2 + count
            || input[2] == 0
        {
            return Err(Error);
        }
        let mut value = 0usize;
        for x in &input[2..2 + count] {
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(*x as usize))
                .ok_or(Error)?;
        }
        if value < 128 {
            return Err(Error);
        }
        (value, 2 + count)
    };
    let end = head.checked_add(len).ok_or(Error)?;
    let data = input.get(head..end).ok_or(Error)?;
    *input = &input[end..];
    Ok(data)
}
fn single<'a>(input: &'a [u8], tag: u8) -> Result<&'a [u8], Error> {
    let mut rest = input;
    let data = take(&mut rest, tag)?;
    if !rest.is_empty() {
        return Err(Error);
    }
    Ok(data)
}
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub state: Option<u8>,
    pub token: Option<Vec<u8>>,
    pub mechanism: Option<Vec<u8>>,
    pub mic: Option<Vec<u8>>,
}
pub fn decode_response(input: &[u8]) -> Result<Response, Error> {
    let mut seq = single(single(input, 0xa1)?, 0x30)?;
    let mut out = Response {
        state: None,
        token: None,
        mechanism: None,
        mic: None,
    };
    let mut last = None;
    while !seq.is_empty() {
        let tag = seq[0];
        if !(0xa0..=0xa3).contains(&tag) || last.is_some_and(|v| tag <= v) {
            return Err(Error);
        }
        last = Some(tag);
        let v = take(&mut seq, tag)?;
        match tag {
            0xa0 => {
                let v = single(v, 0x0a)?;
                if v.len() != 1 || v[0] > 3 {
                    return Err(Error);
                }
                out.state = Some(v[0]);
            }
            0xa1 => {
                let v = single(v, 0x06)?;
                if v != NTLM {
                    return Err(Error);
                }
                out.mechanism = Some(v.to_vec());
            }
            0xa2 => out.token = Some(single(v, 0x04)?.to_vec()),
            0xa3 => out.mic = Some(single(v, 0x04)?.to_vec()),
            _ => return Err(Error),
        }
    }
    Ok(out)
}
