//! Connection configuration is kept here, outside the protocol client.
use super::{client_error, Backend};
use libsmb::{
    auth::ntlm::Credentials,
    client::{Client, Error},
    msg::*,
    DialectCap,
};
use std::{
    net::{SocketAddr, ToSocketAddrs},
    time::Instant,
};

/// One mounted tree. Credentials are retained for reconnect and are not printable.
/// Resolve during mount setup; operation reconnects reuse the successful endpoint.
pub struct Connection {
    client: Client,
    host: String,
    address: SocketAddr,
    share: String,
    user: String,
    domain: String,
    password: String,
    workstation: String,
    dialect: DialectCap,
}
impl Connection {
    pub fn dialect(&self) -> u16 {
        self.client.dialect()
    }
    pub fn encrypted(&self) -> bool {
        self.client.encrypted()
    }
    pub fn connect(
        host: &str,
        port: u16,
        share: &str,
        creds: &Credentials<'_>,
        dialect: DialectCap,
    ) -> Result<Self, Error> {
        let addresses: Vec<_> = (host, port).to_socket_addrs()?.collect();
        let deadline = Instant::now() + super::OPERATION_TIMEOUT;
        let mut last = Error::Protocol;
        for address in addresses {
            match Client::connect_at(host, address, share, creds, dialect, deadline) {
                Ok(client) => {
                    return Ok(Self {
                        client,
                        host: host.into(),
                        address,
                        share: share.into(),
                        user: creds.user.into(),
                        domain: creds.domain.into(),
                        password: creds.password.into(),
                        workstation: creds.workstation.into(),
                        dialect,
                    })
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}
impl Backend for Connection {
    fn set_deadline(&mut self, deadline: Instant) -> Result<(), u32> {
        self.client.set_deadline(Some(deadline));
        Ok(())
    }
    fn reconnect(&mut self, deadline: Instant) -> Result<(), u32> {
        let creds = Credentials {
            user: &self.user,
            domain: &self.domain,
            password: &self.password,
            workstation: &self.workstation,
        };
        self.client = Client::connect_at(
            &self.host,
            self.address,
            &self.share,
            &creds,
            self.dialect,
            deadline,
        )
        .map_err(client_error)?;
        Ok(())
    }
    fn open(
        &mut self,
        path: &str,
        access: u32,
        disposition: CreateDisposition,
        options: u32,
    ) -> Result<FileId, u32> {
        self.client
            .open(path, access, disposition, options)
            .map(|o| o.file_id)
            .map_err(client_error)
    }
    fn close(&mut self, id: FileId) -> Result<(), u32> {
        self.client.close(id).map_err(client_error)
    }
    fn query_file(&mut self, id: FileId) -> Result<FileAllInformation, u32> {
        self.client.query_file(id).map_err(client_error)
    }
    fn list_handle(&mut self, id: FileId) -> Result<Vec<DirectoryEntry>, u32> {
        self.client.list_handle(id).map_err(client_error)
    }
    fn set_info(&mut self, id: FileId, info: SetInfo<'_>) -> Result<(), u32> {
        self.client.set_info(id, info).map_err(client_error)
    }
    fn read_at(&mut self, id: FileId, offset: u64, length: usize) -> Result<Vec<u8>, u32> {
        self.client
            .read_at(id, offset, length)
            .map_err(client_error)
    }
    fn write_at(&mut self, id: FileId, offset: u64, data: &[u8]) -> Result<(), u32> {
        self.client.write_at(id, offset, data).map_err(client_error)
    }
    fn flush(&mut self, id: FileId) -> Result<(), u32> {
        self.client.flush(id).map_err(client_error)
    }
    fn query_fs_size(&mut self, id: FileId) -> Result<FileFsFullSizeInformation, u32> {
        self.client.query_fs_size(id).map_err(client_error)
    }
}
