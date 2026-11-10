//! Blocking SMB client and Direct TCP transport (MS-SMB2 2.1, 3.2.4/3.2.5).
//! Authentication uses MS-NLMP and MS-SPNG through the sans-I/O modules.
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    auth::{
        ntlm::{self, Credentials},
        spnego,
    },
    msg::*,
    session::{Request, Response, Session},
    DialectCap,
};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Protocol,
    Status(NtStatus),
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<crate::Error> for Error {
    fn from(_: crate::Error) -> Self {
        Self::Protocol
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "SMB transport: {:?}", e.kind()),
            Self::Protocol => f.write_str("invalid SMB protocol exchange"),
            Self::Status(s) => write!(f, "SMB NTSTATUS {:08x}", s.0),
        }
    }
}
impl std::error::Error for Error {}

/// One Direct TCP connection, carrying complete SMB messages without framing
/// at this API boundary. The timeout bounds each complete frame, including
/// fragmented headers and bodies. DNS resolution uses the system resolver.
pub struct DirectTcp {
    stream: TcpStream,
    timeout: Duration,
    deadline: Option<Instant>,
}
impl DirectTcp {
    pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<Self, Error> {
        if timeout.is_zero() {
            return Err(Error::Protocol);
        }
        let deadline = Instant::now() + timeout;
        let mut last = io::Error::new(io::ErrorKind::AddrNotAvailable, "no address");
        for address in (host, port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&address, remaining(deadline)?) {
                Ok(stream) => return Self::from_stream(stream, timeout),
                Err(e) => last = e,
            }
        }
        Err(last.into())
    }

    pub fn from_stream(stream: TcpStream, timeout: Duration) -> Result<Self, Error> {
        if timeout.is_zero() {
            return Err(Error::Protocol);
        }
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            timeout,
            deadline: None,
        })
    }

    /// Use the caller's operation deadline for frames, without resetting
    /// it between requests, responses or authentication exchanges.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    fn frame_deadline(&self) -> Instant {
        let frame = Instant::now() + self.timeout;
        self.deadline.map_or(frame, |end| end.min(frame))
    }

    pub fn send(&mut self, message: &[u8]) -> Result<(), Error> {
        let framed = crate::frame(message)?;
        let deadline = self.frame_deadline();
        let mut rest = framed.as_slice();
        while !rest.is_empty() {
            self.stream.set_write_timeout(Some(remaining(deadline)?))?;
            match self.stream.write(rest) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub fn receive(&mut self) -> Result<Vec<u8>, Error> {
        let deadline = self.frame_deadline();
        let mut header = [0; 4];
        self.read_exact(&mut header, deadline)?;
        let size = u32::from_be_bytes(header) as usize;
        // MS-SMB2 2.1: a 0x00 byte and a 24-bit length. Reject malformed
        // framing before allocation; this implementation needs an SMB header.
        if header[0] != 0 || size < 64 {
            return Err(Error::Protocol);
        }
        let mut message = vec![0; size];
        self.read_exact(&mut message, deadline)?;
        Ok(message)
    }

    fn read_exact(&mut self, mut rest: &mut [u8], deadline: Instant) -> Result<(), Error> {
        while !rest.is_empty() {
            self.stream.set_read_timeout(Some(remaining(deadline)?))?;
            match self.stream.read(rest) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
                Ok(n) => rest = &mut rest[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn abort(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}
fn remaining(deadline: Instant) -> Result<Duration, Error> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut).into())
}

/// One authenticated tree, using mandatory signing and share/session encryption.
/// File paths are relative to this tree. Methods close their handles on success
/// and on operation errors; transport/integrity failures terminate the driver.
/// Reconnect and operation replay are the caller's policy.
pub struct Client {
    tcp: DirectTcp,
    session: Session,
}
impl Client {
    pub fn connect(
        host: &str,
        port: u16,
        share: &str,
        creds: &Credentials<'_>,
        max_dialect: DialectCap,
    ) -> Result<Self, Error> {
        let tcp = DirectTcp::connect(host, port, Duration::from_secs(15))?;
        Self::on_transport(host, share, creds, max_dialect, tcp)
    }

    /// Authenticate on an already resolved endpoint under one absolute deadline.
    /// Resolution belongs to the caller and need not run during a reconnect.
    pub fn connect_at(
        host: &str,
        address: SocketAddr,
        share: &str,
        creds: &Credentials<'_>,
        max_dialect: DialectCap,
        deadline: Instant,
    ) -> Result<Self, Error> {
        let stream = TcpStream::connect_timeout(&address, remaining(deadline)?)?;
        let mut tcp = DirectTcp::from_stream(stream, remaining(deadline)?)?;
        tcp.set_deadline(Some(deadline));
        let client = Self::on_transport(host, share, creds, max_dialect, tcp)?;
        remaining(deadline)?;
        Ok(client)
    }

    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.tcp.set_deadline(deadline);
    }

    fn on_transport(
        host: &str,
        share: &str,
        creds: &Credentials<'_>,
        max_dialect: DialectCap,
        tcp: DirectTcp,
    ) -> Result<Self, Error> {
        if host.is_empty()
            || share.is_empty()
            || host.contains(['\\', '/', '\0'])
            || share.contains(['\\', '/', '\0'])
        {
            return Err(Error::Protocol);
        }
        let mut random = [0; 59];
        entropy(&mut random)?;
        let session = Session::new(
            max_dialect,
            random[..16].try_into().map_err(|_| Error::Protocol)?,
            random[16..48].try_into().map_err(|_| Error::Protocol)?,
            random[48..51].try_into().map_err(|_| Error::Protocol)?,
        );
        let mut client = Self { tcp, session };
        client.success(Request::Negotiate)?;
        let negotiate = ntlm::negotiate();
        let token = spnego::encode_init(&negotiate);
        let response = client.exchange(Request::SessionSetup {
            token: &token,
            session_key: None,
        })?;
        if response.header.status != NtStatus::MORE_PROCESSING_REQUIRED.0 {
            return Err(if response.header.status != 0 {
                Error::Status(NtStatus(response.header.status))
            } else {
                Error::Protocol
            });
        }
        let setup = response.decode(SessionSetupResponse::decode)?;
        let challenge = spnego::decode_response(&setup.security_buffer)?;
        if !matches!(challenge.state, Some(1 | 3)) || challenge.mic.is_some() {
            return Err(Error::Protocol);
        }
        let challenge = challenge.token.ok_or(Error::Protocol)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Protocol)?
            .as_nanos()
            / 100;
        let time = u64::try_from(time)
            .ok()
            .and_then(|n| n.checked_add(116_444_736_000_000_000))
            .ok_or(Error::Protocol)?;
        let (authenticate, key) = ntlm::authenticate(
            creds,
            &negotiate,
            &challenge,
            random[51..59].try_into().map_err(|_| Error::Protocol)?,
            time,
        )?;
        let flags = u32::from_le_bytes(
            authenticate[60..64]
                .try_into()
                .map_err(|_| Error::Protocol)?,
        );
        // The mechanism MIC below requires SIGN and extended session security,
        // and this client does not negotiate NTLM key exchange.
        if flags & 0x4008_0010 != 0x0008_0010 {
            return Err(Error::Protocol);
        }
        let token = spnego::encode_authenticated_response(&authenticate, &key);
        let response = client.success(Request::SessionSetup {
            token: &token,
            session_key: Some(key),
        })?;
        let setup = response.decode(SessionSetupResponse::decode)?;
        let final_token = spnego::decode_response(&setup.security_buffer)?;
        if final_token.state != Some(0)
            || !final_token
                .mic
                .as_ref()
                .is_some_and(|mic| spnego::verify_server_mic(&key, mic))
            || final_token.token.as_ref().is_some_and(|t| !t.is_empty())
        {
            return Err(Error::Protocol);
        }
        client.success(Request::TreeConnect(&format!("\\\\{host}\\{share}")))?;
        if client.dialect() == 0x0302 {
            client.success(Request::ValidateNegotiate)?;
        }
        Ok(client)
    }

    pub fn encrypted(&self) -> bool {
        self.session.encrypted()
    }

    pub fn dialect(&self) -> u16 {
        self.session.negotiated().map_or(0, |n| n.dialect)
    }

    /// Transfer ownership to a caller that needs the lower-level protocol API.
    pub fn into_parts(self) -> (DirectTcp, Session) {
        (self.tcp, self.session)
    }

    fn exchange(&mut self, request: Request<'_>) -> Result<Response, Error> {
        let wire = self.session.build(request)?;
        let result = (|| {
            self.tcp.send(&wire)?;
            loop {
                let wire = self.tcp.receive()?;
                if let Some(response) = self.session.consume(&wire)? {
                    return Ok(response);
                }
            }
        })();
        if result.is_err() {
            self.session.close();
            self.tcp.abort();
        }
        result
    }
    fn success(&mut self, request: Request<'_>) -> Result<Response, Error> {
        let r = self.exchange(request)?;
        if r.header.status != 0 {
            return Err(Error::Status(NtStatus(r.header.status)));
        }
        Ok(r)
    }
    pub fn open(
        &mut self,
        path: &str,
        access: u32,
        disposition: CreateDisposition,
        options: u32,
    ) -> Result<CreateResponse, Error> {
        let name = relative_path(path)?;
        let response = self.success(Request::Create(CreateRequest {
            name: &name,
            oplock_level: 0,
            impersonation_level: 2,
            desired_access: access,
            file_attributes: 0,
            share_access: 7,
            disposition,
            options,
            contexts: &[],
        }))?;
        Ok(response.decode(CreateResponse::decode)?)
    }
    pub fn close(&mut self, id: FileId) -> Result<(), Error> {
        self.success(Request::Close(CloseRequest {
            file_id: id,
            postquery_attributes: false,
        }))?
        .decode(CloseResponse::decode)?;
        Ok(())
    }
    fn with_file<T>(
        &mut self,
        opened: CreateResponse,
        operation: impl FnOnce(&mut Self, &CreateResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = operation(self, &opened);
        let closed = self.close(opened.file_id);
        match result {
            Ok(value) => closed.map(|()| value),
            Err(e) => Err(e),
        }
    }
    fn chunk_size(&self, maximum: u32) -> Result<u32, Error> {
        let n = self.session.negotiated().ok_or(Error::Protocol)?;
        let credits = if n.capabilities & 4 == 0 {
            self.session.credits().min(1)
        } else {
            self.session.credits().min(u64::from(u16::MAX))
        };
        // Leave room for SMB/transform headers in the 24-bit Direct TCP length.
        let size = u64::from(maximum).min(credits * 65536).min(0xff_ffff - 256);
        if size == 0 {
            return Err(Error::Protocol);
        }
        Ok(size as u32)
    }

    pub fn list_handle(&mut self, id: FileId) -> Result<Vec<DirectoryEntry>, Error> {
        let mut entries = Vec::new();
        let mut first = true;
        loop {
            let output_length = self
                .chunk_size(
                    self.session
                        .negotiated()
                        .ok_or(Error::Protocol)?
                        .max_transact_size,
                )?
                .min(65536);
            let r = self.exchange(Request::QueryDirectory(QueryDirectoryRequest {
                file_id: id,
                pattern: "*",
                output_length,
                flags: u8::from(first),
            }))?;
            if r.header.status == NtStatus::NO_MORE_FILES.0 {
                break;
            }
            if r.header.status != 0 {
                return Err(Error::Status(NtStatus(r.header.status)));
            }
            let batch = r.decode(QueryDirectoryResponse::decode)?.entries;
            if batch.is_empty() {
                return Err(Error::Protocol);
            }
            entries.extend(
                batch
                    .into_iter()
                    .filter(|e| e.name != "." && e.name != ".."),
            );
            first = false;
        }
        Ok(entries)
    }
    pub fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>, Error> {
        let opened = self.open(
            path,
            access_mask::LIST_DIRECTORY,
            CreateDisposition::Open,
            create_options::DIRECTORY_FILE,
        )?;
        self.with_file(opened, |c, o| c.list_handle(o.file_id))
    }
    pub fn read_at(&mut self, id: FileId, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        let mut data = Vec::new();
        while data.len() < length {
            let size = (self.chunk_size(
                self.session
                    .negotiated()
                    .ok_or(Error::Protocol)?
                    .max_read_size,
            )? as usize)
                .min(length - data.len());
            let at = offset
                .checked_add(data.len() as u64)
                .ok_or(Error::Protocol)?;
            let r = self.exchange(Request::Read(ReadRequest {
                file_id: id,
                offset: at,
                length: size as u32,
            }))?;
            if r.header.status == NtStatus::END_OF_FILE.0 {
                break;
            }
            if r.header.status != 0 {
                return Err(Error::Status(NtStatus(r.header.status)));
            }
            let part = r.decode(ReadResponse::decode)?.data;
            if part.is_empty() || part.len() > size {
                return Err(Error::Protocol);
            }
            data.try_reserve(part.len()).map_err(|_| Error::Protocol)?;
            data.extend_from_slice(&part);
        }
        Ok(data)
    }
    pub fn write_at(&mut self, id: FileId, offset: u64, data: &[u8]) -> Result<(), Error> {
        let mut written = 0;
        while written < data.len() {
            let size = (self.chunk_size(
                self.session
                    .negotiated()
                    .ok_or(Error::Protocol)?
                    .max_write_size,
            )? as usize)
                .min(data.len() - written);
            let at = offset.checked_add(written as u64).ok_or(Error::Protocol)?;
            let r = self.success(Request::Write(WriteRequest {
                file_id: id,
                offset: at,
                data: &data[written..written + size],
            }))?;
            let count = r.decode(WriteResponse::decode)?.count as usize;
            if count == 0 || count > size {
                return Err(Error::Protocol);
            }
            written += count;
        }
        Ok(())
    }
    pub fn query_file(&mut self, id: FileId) -> Result<FileAllInformation, Error> {
        Ok(self
            .success(Request::QueryInfo(QueryInfoRequest {
                file_id: id,
                output_length: 65536,
            }))?
            .decode(QueryInfoResponse::decode)?
            .info)
    }
    pub fn query_fs_size(&mut self, id: FileId) -> Result<FileFsFullSizeInformation, Error> {
        Ok(self
            .success(Request::QueryFsSize(QueryFsSizeRequest { file_id: id }))?
            .decode(QueryFsSizeResponse::decode)?
            .info)
    }
    pub fn set_info(&mut self, id: FileId, info: SetInfo<'_>) -> Result<(), Error> {
        self.success(Request::SetInfo(SetInfoRequest { file_id: id, info }))?
            .decode(decode_set_info_response)?;
        Ok(())
    }
    pub fn flush(&mut self, id: FileId) -> Result<(), Error> {
        self.success(Request::Flush(id))?
            .decode(decode_flush_response)?;
        Ok(())
    }
    pub fn read(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        let opened = self.open(
            path,
            access_mask::READ_DATA,
            CreateDisposition::Open,
            create_options::NON_DIRECTORY_FILE,
        )?;
        self.with_file(opened, |c, o| {
            c.read_at(
                o.file_id,
                0,
                usize::try_from(o.attributes.end_of_file).map_err(|_| Error::Protocol)?,
            )
        })
    }
    /// Create or truncate a file, write the supplied bytes, flush, then close it.
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        let opened = self.open(
            path,
            access_mask::WRITE_DATA,
            CreateDisposition::OverwriteIf,
            create_options::NON_DIRECTORY_FILE,
        )?;
        self.with_file(opened, |c, o| {
            c.write_at(o.file_id, 0, data)?;
            c.flush(o.file_id)
        })
    }
    pub fn rename_replace(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let name = relative_path(to)?;
        let opened = self.open(
            from,
            access_mask::DELETE,
            CreateDisposition::Open,
            create_options::NON_DIRECTORY_FILE,
        )?;
        self.with_file(opened, |c, opened| {
            c.set_info(
                opened.file_id,
                SetInfo::Rename {
                    replace_if_exists: true,
                    name: &name,
                },
            )
        })
    }
    pub fn delete(&mut self, path: &str) -> Result<(), Error> {
        let opened = self.open(
            path,
            access_mask::DELETE,
            CreateDisposition::Open,
            create_options::NON_DIRECTORY_FILE,
        )?;
        self.with_file(opened, |c, opened| {
            c.set_info(opened.file_id, SetInfo::Disposition(true))
        })
    }
}
fn relative_path(path: &str) -> Result<String, Error> {
    let path = path.replace('/', "\\");
    if path.starts_with('\\') || path.contains(['\0', ':']) || path.split('\\').any(|c| c == "..") {
        return Err(Error::Protocol);
    }
    Ok(path)
}
fn entropy(bytes: &mut [u8]) -> Result<(), Error> {
    #[cfg(target_os = "breenix")]
    {
        let mut at = 0;
        while at < bytes.len() {
            let n = libbreenix::crypto::rand::getrandom_bytes(&mut bytes[at..])
                .map_err(|_| Error::Protocol)?;
            if n == 0 || n > bytes.len() - at {
                return Err(Error::Protocol);
            }
            at += n;
        }
        Ok(())
    }
    #[cfg(not(target_os = "breenix"))]
    {
        std::fs::File::open("/dev/urandom")?.read_exact(bytes)?;
        Ok(())
    }
}
