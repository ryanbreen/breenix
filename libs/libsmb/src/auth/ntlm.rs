//! NTLMv2 connection-oriented authentication (MS-NLMP).
use crate::Error;
use libbreenix::crypto::{hmac::hmac_md5, md4::md4};

const FLAGS: u32 = 0xa088_8215; // Unicode, NTLM, extended security, target info, 128/56-bit
const KEY_EXCH: u32 = 0x4000_0000;

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

pub fn ntowfv2(password: &str, user: &str, domain: &str) -> [u8; 16] {
    let hash = md4(&utf16(password));
    hmac_md5(&hash, &utf16(&(user.to_uppercase() + domain)))
}

pub fn ntlmv2_response(
    key: &[u8; 16],
    server: [u8; 8],
    client: [u8; 8],
    time: u64,
    target: &[u8],
) -> (Vec<u8>, [u8; 16]) {
    let mut blob = vec![1, 1, 0, 0, 0, 0, 0, 0];
    blob.extend_from_slice(&time.to_le_bytes());
    blob.extend_from_slice(&client);
    blob.extend_from_slice(&[0; 4]);
    blob.extend_from_slice(target);
    blob.extend_from_slice(&[0; 4]);
    let mut input = server.to_vec();
    input.extend_from_slice(&blob);
    let proof = hmac_md5(key, &input);
    let session = hmac_md5(key, &proof);
    let mut response = proof.to_vec();
    response.extend_from_slice(&blob);
    (response, session)
}

pub fn negotiate() -> Vec<u8> {
    let mut b = b"NTLMSSP\0\x01\0\0\0".to_vec();
    b.extend_from_slice(&(FLAGS & !KEY_EXCH).to_le_bytes());
    b.extend_from_slice(&[0; 16]);
    b
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AvPair {
    pub id: u16,
    pub value: Vec<u8>,
}

pub fn decode_av_pairs(mut b: &[u8]) -> Result<Vec<AvPair>, Error> {
    let mut out = Vec::new();
    loop {
        if b.len() < 4 {
            return Err(Error);
        }
        let id = u16::from_le_bytes([b[0], b[1]]);
        let len = u16::from_le_bytes([b[2], b[3]]) as usize;
        b = &b[4..];
        if id == 0 {
            return if len == 0 && b.is_empty() {
                Ok(out)
            } else {
                Err(Error)
            };
        }
        if b.len() < len || (id == 6 && len != 4) || (id == 7 && len != 8) {
            return Err(Error);
        }
        out.push(AvPair {
            id,
            value: b[..len].to_vec(),
        });
        b = &b[len..];
    }
}

fn encode_av_pairs(pairs: &[AvPair]) -> Result<Vec<u8>, Error> {
    let mut b = Vec::new();
    for p in pairs {
        if p.id == 0 {
            return Err(Error);
        }
        let len = u16::try_from(p.value.len()).map_err(|_| Error)?;
        b.extend_from_slice(&p.id.to_le_bytes());
        b.extend_from_slice(&len.to_le_bytes());
        b.extend_from_slice(&p.value);
    }
    b.extend_from_slice(&[0; 4]);
    Ok(b)
}

#[derive(Debug, Clone)]
pub struct Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    pub target_info: Vec<AvPair>,
}
fn field(b: &[u8], pos: usize) -> Result<&[u8], Error> {
    let f = b.get(pos..pos + 8).ok_or(Error)?;
    let len = u16::from_le_bytes([f[0], f[1]]) as usize;
    let max = u16::from_le_bytes([f[2], f[3]]) as usize;
    let start = u32::from_le_bytes(f[4..8].try_into().map_err(|_| Error)?) as usize;
    if max < len {
        return Err(Error);
    }
    b.get(start..start.checked_add(len).ok_or(Error)?)
        .ok_or(Error)
}
pub fn decode_challenge(b: &[u8]) -> Result<Challenge, Error> {
    if b.len() < 48 || &b[..12] != b"NTLMSSP\0\x02\0\0\0" {
        return Err(Error);
    }
    let flags = u32::from_le_bytes(b[20..24].try_into().map_err(|_| Error)?);
    if flags & 1 == 0 || flags & 0x80000 == 0 || flags & 0x800000 == 0 {
        return Err(Error);
    }
    let _ = field(b, 12)?;
    Ok(Challenge {
        flags,
        server_challenge: b[24..32].try_into().map_err(|_| Error)?,
        target_info: decode_av_pairs(field(b, 40)?)?,
    })
}

/// Credentials are intentionally not printable, including through Debug.
pub struct Credentials<'a> {
    pub user: &'a str,
    pub domain: &'a str,
    pub password: &'a str,
    pub workstation: &'a str,
}
impl core::fmt::Debug for Credentials<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("domain", &self.domain)
            .field("password", &"[redacted]")
            .field("workstation", &self.workstation)
            .finish()
    }
}

fn put_field(b: &mut [u8], at: usize, len: usize, offset: usize) -> Result<(), Error> {
    let len = u16::try_from(len).map_err(|_| Error)?;
    let offset = u32::try_from(offset).map_err(|_| Error)?;
    b[at..at + 2].copy_from_slice(&len.to_le_bytes());
    b[at + 2..at + 4].copy_from_slice(&len.to_le_bytes());
    b[at + 4..at + 8].copy_from_slice(&offset.to_le_bytes());
    Ok(())
}

pub fn authenticate(
    creds: &Credentials<'_>,
    negotiate_message: &[u8],
    challenge_message: &[u8],
    client: [u8; 8],
    now: u64,
) -> Result<(Vec<u8>, [u8; 16]), Error> {
    let challenge = decode_challenge(challenge_message)?;
    let mut pairs = challenge.target_info;
    let timestamp = pairs
        .iter()
        .find(|p| p.id == 7)
        .map(|p| u64::from_le_bytes(p.value.as_slice().try_into().expect("validated timestamp")));
    if timestamp.is_some() {
        if let Some(p) = pairs.iter_mut().find(|p| p.id == 6) {
            let mut flags = u32::from_le_bytes(p.value.as_slice().try_into().map_err(|_| Error)?);
            flags |= 2;
            p.value = flags.to_le_bytes().to_vec();
        } else {
            pairs.push(AvPair {
                id: 6,
                value: 2u32.to_le_bytes().to_vec(),
            });
        }
    }
    let target = encode_av_pairs(&pairs)?;
    let key = ntowfv2(creds.password, creds.user, creds.domain);
    let (nt, session) = ntlmv2_response(
        &key,
        challenge.server_challenge,
        client,
        timestamp.unwrap_or(now),
        &target,
    );
    let lm = if timestamp.is_some() {
        vec![0; 24]
    } else {
        let mut x = challenge.server_challenge.to_vec();
        x.extend_from_slice(&client);
        let mut r = hmac_md5(&key, &x).to_vec();
        r.extend_from_slice(&client);
        r
    };
    let domain = utf16(creds.domain);
    let user = utf16(creds.user);
    let workstation = utf16(creds.workstation);
    let mut b = vec![0; if timestamp.is_some() { 80 } else { 64 }];
    b[..12].copy_from_slice(b"NTLMSSP\0\x03\0\0\0");
    let flags = challenge.flags & FLAGS & !KEY_EXCH;
    b[60..64].copy_from_slice(&flags.to_le_bytes());
    for (at, data) in [
        (12, lm.as_slice()),
        (20, nt.as_slice()),
        (28, domain.as_slice()),
        (36, user.as_slice()),
        (44, workstation.as_slice()),
        (52, &[][..]),
    ] {
        let offset = b.len();
        put_field(&mut b, at, data.len(), offset)?;
        b.extend_from_slice(data);
    }
    if timestamp.is_some() {
        let mut input = negotiate_message.to_vec();
        input.extend_from_slice(challenge_message);
        input.extend_from_slice(&b);
        let mic = hmac_md5(&session, &input);
        b[64..80].copy_from_slice(&mic);
    }
    Ok((b, session))
}
