//! One connection, authenticated session and tree, without transport I/O.
//!
//! MS-SMB2 3.2.4/3.2.5. Messages exclude Direct TCP framing. The caller
//! exchanges SPNEGO tokens through `auth` and supplies the exported session key
//! with the last SESSION_SETUP request. Binding, guest access, compounds,
//! unsolicited oplock breaks and durable reconnects are not supported.

use std::collections::BTreeMap;

use crate::{
    decode_negotiate_response, encode_negotiate_request,
    msg::*,
    security::{derive_keys, sign, verify, Dialect, PreauthHash, SessionKeys},
    transform::{Cipher, Transform},
    u16_at, u32_at, u64_at, DialectCap, Error, NegotiateContext, NegotiateResponse, SyncHeader,
};

/// Requests take operation data; the driver supplies protocol identifiers,
/// credits and security fields. Call ValidateNegotiate after TREE_CONNECT on
/// SMB 3.0.2, before performing file operations.
pub enum Request<'a> {
    Negotiate,
    SessionSetup {
        token: &'a [u8],
        session_key: Option<[u8; 16]>,
    },
    TreeConnect(&'a str),
    ValidateNegotiate,
    TreeDisconnect,
    Logoff,
    Create(CreateRequest<'a>),
    Close(CloseRequest),
    Read(ReadRequest),
    Write(WriteRequest<'a>),
    Flush(FileId),
    QueryDirectory(QueryDirectoryRequest<'a>),
    QueryInfo(QueryInfoRequest),
    QueryFsSize(QueryFsSizeRequest),
    SetInfo(SetInfoRequest<'a>),
}

/// A correlated response. `message` retains the original decrypted SMB bytes
/// over which integrity was checked. `header.tree_id` comes from the saved
/// request when an asynchronous header carries AsyncId instead of TreeId.
#[derive(Debug)]
pub struct Response {
    pub header: SyncHeader,
    pub async_id: Option<u64>,
    pub message: Vec<u8>,
}

impl Response {
    /// Use the synchronous body codecs on an already verified async response.
    /// The temporary decoder view replaces AsyncId with the saved TreeId;
    /// use `message` for signature verification or wire diagnostics.
    pub fn decode<T>(&self, decoder: impl FnOnce(&[u8]) -> Result<T, Error>) -> Result<T, Error> {
        if self.async_id.is_none() {
            return decoder(&self.message);
        }
        let mut view = self.message.clone();
        view[16..20].copy_from_slice(&(self.header.flags & !2).to_le_bytes());
        view[32..36].fill(0);
        view[36..40].copy_from_slice(&self.header.tree_id.to_le_bytes());
        decoder(&view)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    New,
    Negotiating,
    Authenticating,
    Established,
    Closed,
}

struct Outstanding {
    command: u16,
    tree_id: u32,
    encrypted: bool,
    async_id: Option<u64>,
}

pub struct Session {
    cap: DialectCap,
    guid: [u8; 16],
    salt: [u8; 32],
    nonce_prefix: [u8; 3],
    phase: Phase,
    next_id: u64,
    credits: u64,
    outstanding: BTreeMap<u64, Outstanding>,
    negotiated: Option<NegotiateResponse>,
    preauth: PreauthHash,
    session_id: u64,
    tree_id: Option<u32>,
    session_encrypt: bool,
    tree_encrypt: bool,
    validated: bool,
    session_key: Option<[u8; 16]>,
    keys: Option<SessionKeys>,
    cipher: Option<Cipher>,
    outgoing: Option<Transform>,
    incoming: Option<Transform>,
}

impl Session {
    /// Supply a distinct nonce prefix for connections sharing a session key,
    /// including reconnections, as required by Transform::new.
    pub fn new(cap: DialectCap, guid: [u8; 16], salt: [u8; 32], nonce_prefix: [u8; 3]) -> Self {
        Self {
            cap,
            guid,
            salt,
            nonce_prefix,
            phase: Phase::New,
            next_id: 0,
            credits: 1,
            outstanding: BTreeMap::new(),
            negotiated: None,
            preauth: PreauthHash::new(),
            session_id: 0,
            tree_id: None,
            session_encrypt: false,
            tree_encrypt: false,
            validated: false,
            session_key: None,
            keys: None,
            cipher: None,
            outgoing: None,
            incoming: None,
        }
    }

    pub fn credits(&self) -> u64 {
        self.credits
    }
    pub fn next_message_id(&self) -> u64 {
        self.next_id
    }
    pub fn session_id(&self) -> u64 {
        self.session_id
    }
    pub fn tree_id(&self) -> Option<u32> {
        self.tree_id
    }
    pub fn preauth_hash(&self) -> &[u8; 64] {
        self.preauth.value()
    }
    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }
    pub fn negotiated(&self) -> Option<&NegotiateResponse> {
        self.negotiated.as_ref()
    }

    /// Abandon this connection after a transport failure. A new connection uses
    /// a new driver; requests are not silently replayed across sessions.
    pub fn close(&mut self) {
        self.phase = Phase::Closed;
        self.outstanding.clear();
        self.tree_id = None;
        self.session_id = 0;
        self.keys = None;
        self.session_key = None;
        self.outgoing = None;
        self.incoming = None;
        self.credits = 0;
    }

    pub fn build(&mut self, request: Request<'_>) -> Result<Vec<u8>, Error> {
        if self.is_closed() {
            return Err(Error);
        }
        let mut h = SyncHeader {
            credit_charge: 1,
            status: 0,
            command: 0,
            credits: 32,
            flags: 0,
            next_command: 0,
            message_id: self.next_id,
            tree_id: self.tree_id.unwrap_or(0),
            session_id: self.session_id,
            signature: [0; 16],
        };
        let mut exported_key = None;
        let mut message = match request {
            Request::Negotiate => {
                if self.phase != Phase::New {
                    return Err(Error);
                }
                let mut b = encode_negotiate_request(self.cap, self.guid, self.salt);
                // Advertise LARGE_MTU for u3's multi-credit transfers and
                // require signing. Preserve these same values in validation.
                b[14..16].copy_from_slice(&h.credits.to_le_bytes());
                b[68..70].copy_from_slice(&3u16.to_le_bytes());
                b[72..76].copy_from_slice(&0x44u32.to_le_bytes());
                b
            }
            Request::SessionSetup { token, session_key } => {
                if self.phase != Phase::Authenticating || !self.outstanding.is_empty() {
                    return Err(Error);
                }
                exported_key = session_key;
                h.tree_id = 0;
                SessionSetupRequest {
                    flags: 0,
                    security_mode: 3,
                    capabilities: 0,
                    previous_session_id: 0,
                    security_buffer: token,
                }
                .encode(&h)?
            }
            request => {
                if self.phase != Phase::Established {
                    return Err(Error);
                }
                match request {
                    Request::TreeConnect(path) => {
                        if self.tree_id.is_some() || !self.outstanding.is_empty() {
                            return Err(Error);
                        }
                        TreeConnectRequest { flags: 0, path }.encode(&h)?
                    }
                    Request::Logoff => {
                        if !self.outstanding.is_empty() {
                            return Err(Error);
                        }
                        h.tree_id = 0;
                        encode_logoff_request(&h)?
                    }
                    request => {
                        if self.tree_id.is_none() {
                            return Err(Error);
                        }
                        match request {
                            Request::ValidateNegotiate => {
                                if self.dialect()? != Dialect::Smb302
                                    || self.validated
                                    || !self.outstanding.is_empty()
                                {
                                    return Err(Error);
                                }
                                ValidateNegotiateRequest {
                                    capabilities: 0x44,
                                    guid: self.guid,
                                    security_mode: 3,
                                    dialects: if self.cap == DialectCap::Smb302 {
                                        &[0x0302]
                                    } else {
                                        &[0x0302, 0x0311]
                                    },
                                }
                                .encode(&h)?
                            }
                            Request::TreeDisconnect => {
                                if !self.outstanding.is_empty() {
                                    return Err(Error);
                                }
                                encode_tree_disconnect_request(&h)?
                            }
                            request => {
                                if !self.validated
                                    || self
                                        .outstanding
                                        .values()
                                        .any(|r| matches!(r.command, 2..=4 | 11))
                                {
                                    return Err(Error);
                                }
                                match request {
                                    Request::Create(r) => r.encode(&h)?,
                                    Request::Close(r) => r.encode(&h)?,
                                    Request::Read(r) => r.encode(&h)?,
                                    Request::Write(r) => r.encode(&h)?,
                                    Request::Flush(id) => encode_flush_request(&h, id)?,
                                    Request::QueryDirectory(r) => r.encode(&h)?,
                                    Request::QueryInfo(r) => r.encode(&h)?,
                                    Request::QueryFsSize(r) => r.encode(&h)?,
                                    Request::SetInfo(r) => r.encode(&h)?,
                                    _ => return Err(Error),
                                }
                            }
                        }
                    }
                }
            }
        };
        let command = u16_at(&message, 12)?;
        let charge = self.charge(&message)?;
        let next_id = self
            .next_id
            .checked_add(charge)
            .filter(|id| *id < u64::MAX)
            .ok_or(Error)?;
        if charge > self.credits {
            return Err(Error);
        }
        let multi_credit = self
            .negotiated
            .as_ref()
            .is_some_and(|n| n.capabilities & 4 != 0);
        message[6..8]
            .copy_from_slice(&(if multi_credit { charge as u16 } else { 0 }).to_le_bytes());
        let encrypted = command > 1 && (self.session_encrypt || self.tree_encrypt);
        if command > 1 && (!encrypted || command == 11) {
            sign(&self.keys.as_ref().ok_or(Error)?.signing, &mut message)?;
        }
        let wire = if encrypted {
            self.outgoing.as_mut().ok_or(Error)?.encrypt(&message)?
        } else {
            message.clone()
        };
        // Commit state only once the complete request has been constructed.
        if command <= 1 {
            self.preauth.update(&message);
        }
        if command == 0 {
            self.phase = Phase::Negotiating;
        }
        if exported_key.is_some() {
            self.session_key = exported_key;
        }
        self.credits -= charge;
        self.next_id = next_id;
        self.outstanding.insert(
            h.message_id,
            Outstanding {
                command,
                tree_id: h.tree_id,
                encrypted,
                async_id: None,
            },
        );
        Ok(wire)
    }

    fn dialect(&self) -> Result<Dialect, Error> {
        match self.negotiated.as_ref().ok_or(Error)?.dialect {
            0x0302 => Ok(Dialect::Smb302),
            0x0311 => Ok(Dialect::Smb311),
            _ => Err(Error),
        }
    }

    fn charge(&self, b: &[u8]) -> Result<u64, Error> {
        let Some(n) = &self.negotiated else {
            return Ok(1);
        };
        let command = u16_at(b, 12)?;
        let (size, maximum) = match command {
            8 => (u32_at(b, 68)?, n.max_read_size),
            9 => (u32_at(b, 68)?, n.max_write_size),
            11 => (u32_at(b, 92)?.max(u32_at(b, 108)?), n.max_transact_size),
            14 => (u32_at(b, 92)?, n.max_transact_size),
            16 | 17 => (u32_at(b, 68)?, n.max_transact_size),
            _ => return Ok(1),
        };
        let charge = credit_charge(size);
        if size > maximum || charge > u16::MAX as u32 || (charge > 1 && n.capabilities & 4 == 0) {
            return Err(Error);
        }
        Ok(u64::from(charge))
    }

    /// Consume one complete response. Pending responses grant their credits
    /// once and are skipped; the eventual final response completes the request.
    /// Invalid integrity/correlation leaves the outstanding request untouched.
    pub fn consume(&mut self, wire: &[u8]) -> Result<Option<Response>, Error> {
        if self.is_closed() {
            return Err(Error);
        }
        let encrypted = wire.starts_with(b"\xfdSMB");
        let message = if encrypted {
            self.incoming.as_ref().ok_or(Error)?.decrypt(wire)?
        } else {
            wire.to_vec()
        };
        let flags = u32_at(&message, 16)?;
        let async_id = if flags & 2 != 0 {
            Some(u64_at(&message, 32)?)
        } else {
            None
        };
        let mut header_bytes: [u8; 64] = message
            .get(..64)
            .ok_or(Error)?
            .try_into()
            .map_err(|_| Error)?;
        header_bytes[16..20].copy_from_slice(&(flags & !2).to_le_bytes());
        let mut header = SyncHeader::decode(&header_bytes)?;
        header.flags = flags;
        if flags & 1 == 0 || flags & 0x0fff_ff80 != 0 || header.next_command != 0 {
            return Err(Error);
        }
        let request = self.outstanding.get(&header.message_id).ok_or(Error)?;
        if header.command != request.command
            || (header.command != 1 && header.session_id != self.session_id)
            || (header.command == 1
                && (header.session_id == 0
                    || (self.session_id != 0 && header.session_id != self.session_id)))
            || (request.encrypted && !encrypted)
        {
            return Err(Error);
        }
        if async_id.is_some() {
            header.tree_id = request.tree_id;
        }
        if header.command != 3 && header.command > 1 && header.tree_id != request.tree_id {
            return Err(Error);
        }
        let pending = header.status == NtStatus::PENDING.0;
        if pending {
            if async_id.is_none()
                || async_id == Some(0)
                || request.async_id.is_some()
                || self.outstanding.values().any(|r| r.async_id == async_id)
            {
                return Err(Error);
            }
        } else if async_id != request.async_id {
            return Err(Error);
        }

        let final_setup = header.command == 1 && header.status == 0;
        let setup_keys = if final_setup {
            Some(derive_keys(
                self.dialect()?,
                self.session_key.as_ref().ok_or(Error)?,
                self.preauth.value(),
            ))
        } else {
            None
        };
        let signing = setup_keys.as_ref().or(self.keys.as_ref());
        if !encrypted
            && !pending
            && (self.phase == Phase::Established || final_setup)
            && !verify(&signing.ok_or(Error)?.signing, &message)
        {
            return Err(Error);
        }
        let credits = self
            .credits
            .checked_add(u64::from(header.credits))
            .ok_or(Error)?;
        self.next_id
            .checked_add(credits)
            .filter(|id| *id < u64::MAX)
            .ok_or(Error)?;
        let response = Response {
            header,
            async_id,
            message,
        };
        if pending {
            response.decode(ErrorResponse::decode)?;
            self.outstanding
                .get_mut(&response.header.message_id)
                .ok_or(Error)?
                .async_id = async_id;
            self.credits = credits;
            return Ok(None);
        }
        // A failed validate-negotiate terminates this driver, including malformed
        // replies; normal SMB operation errors remain visible to the caller.
        let applied = self.apply_response(&response, setup_keys);
        if applied.is_err() && response.header.command == 11 {
            self.close();
        }
        applied?;
        self.outstanding.remove(&response.header.message_id);
        if !self.is_closed() {
            self.credits = credits;
        }
        Ok(Some(response))
    }

    fn apply_response(
        &mut self,
        r: &Response,
        setup_keys: Option<SessionKeys>,
    ) -> Result<(), Error> {
        let command = r.header.command;
        let status = NtStatus(r.header.status);
        if status != NtStatus::SUCCESS
            && !(command == 1 && status == NtStatus::MORE_PROCESSING_REQUIRED)
        {
            if command == 11 {
                return Err(Error);
            }
            // Warning statuses can carry command-specific output rather than
            // an ERROR body (MS-SMB2 3.2.5.12/15). Preserve it for the caller.
            if command <= 1 {
                r.decode(ErrorResponse::decode)?;
                self.close();
            }
            return Ok(());
        }
        match command {
            0 => {
                let n = r.decode(decode_negotiate_response)?;
                let offered =
                    n.dialect == 0x0302 || (n.dialect == 0x0311 && self.cap == DialectCap::Default);
                if !offered
                    || n.security_mode & 1 == 0
                    || n.max_read_size == 0
                    || n.max_write_size == 0
                    || n.max_transact_size == 0
                {
                    return Err(Error);
                }
                let cipher = if n.dialect == 0x0311 {
                    if n.contexts.iter().filter(|c| matches!(c, NegotiateContext::Preauth { .. })).count() != 1
                        || !n.contexts.iter().any(|c| matches!(c, NegotiateContext::Preauth { algorithms, .. } if algorithms == &[1]))
                        || n.contexts.iter().filter(|c| matches!(c, NegotiateContext::Encryption { .. })).count() > 1
                    { return Err(Error); }
                    match n
                        .contexts
                        .iter()
                        .find(|c| matches!(c, NegotiateContext::Encryption { .. }))
                    {
                        Some(NegotiateContext::Encryption { ciphers }) if ciphers == &[2] => {
                            Some(Cipher::Aes128Gcm)
                        }
                        Some(NegotiateContext::Encryption { ciphers }) if ciphers == &[1] => {
                            Some(Cipher::Aes128Ccm)
                        }
                        None => None,
                        _ => return Err(Error),
                    }
                } else if n.capabilities & 0x40 != 0 {
                    Some(Cipher::Aes128Ccm)
                } else {
                    None
                };
                self.cipher = cipher;
                self.preauth.update(&r.message);
                self.negotiated = Some(n);
                self.phase = Phase::Authenticating;
            }
            1 => {
                let setup = r.decode(SessionSetupResponse::decode)?;
                if status == NtStatus::MORE_PROCESSING_REQUIRED {
                    self.preauth.update(&r.message);
                    self.session_id = r.header.session_id;
                } else {
                    if setup.session_flags & (session_flags::IS_GUEST | session_flags::IS_NULL) != 0
                        || (setup.session_flags & session_flags::ENCRYPT_DATA != 0
                            && self.cipher.is_none())
                    {
                        self.close();
                        return Err(Error);
                    }
                    let keys = setup_keys.ok_or(Error)?;
                    if let Some(cipher) = self.cipher {
                        self.outgoing = Some(Transform::new(
                            cipher,
                            keys.encryption,
                            r.header.session_id,
                            self.nonce_prefix,
                        ));
                        self.incoming = Some(Transform::new(
                            cipher,
                            keys.decryption,
                            r.header.session_id,
                            [0; 3],
                        ));
                    }
                    self.session_id = r.header.session_id;
                    self.session_encrypt = setup.session_flags & session_flags::ENCRYPT_DATA != 0;
                    self.validated = self.dialect()? == Dialect::Smb311;
                    self.keys = Some(keys);
                    self.session_key = None;
                    self.phase = Phase::Established;
                }
            }
            2 => {
                r.decode(decode_logoff_response)?;
                self.close();
            }
            3 => {
                let tree = r.decode(TreeConnectResponse::decode)?;
                if tree.header.tree_id == 0
                    || (tree.share_flags & share_flags::ENCRYPT_DATA != 0 && self.cipher.is_none())
                {
                    self.close();
                    return Err(Error);
                }
                self.tree_id = Some(tree.header.tree_id);
                self.tree_encrypt = tree.share_flags & share_flags::ENCRYPT_DATA != 0;
                self.validated = self.dialect()? == Dialect::Smb311;
            }
            4 => {
                r.decode(decode_tree_disconnect_response)?;
                self.tree_id = None;
                self.tree_encrypt = false;
                self.validated = false;
            }
            11 => {
                let v = r.decode(ValidateNegotiateResponse::decode)?;
                let n = self.negotiated.as_ref().ok_or(Error)?;
                if v.capabilities != n.capabilities
                    || v.guid != n.server_guid
                    || v.security_mode != n.security_mode
                    || v.dialect != n.dialect
                {
                    return Err(Error);
                }
                self.validated = true;
            }
            _ => {}
        }
        Ok(())
    }
}
