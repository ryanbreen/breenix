//! Blocking SMB client and Direct TCP transport (MS-SMB2).
use crate::{auth::ntlm::Credentials, msg::DirectoryEntry, session::Session, DialectCap};
use std::{io, time::Duration};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Protocol,
    Status(crate::msg::NtStatus),
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

pub struct DirectTcp;
impl DirectTcp {
    pub fn connect(_host: &str, _port: u16, _timeout: Duration) -> Result<Self, Error> {
        Err(Error::Protocol)
    }
    pub fn send(&mut self, _message: &[u8]) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    pub fn receive(&mut self) -> Result<Vec<u8>, Error> {
        Err(Error::Protocol)
    }
}
pub struct Client;
impl Client {
    pub fn connect(
        _host: &str,
        _port: u16,
        _share: &str,
        _creds: &Credentials<'_>,
        _max_dialect: DialectCap,
    ) -> Result<Self, Error> {
        Err(Error::Protocol)
    }
    pub fn dialect(&self) -> u16 {
        0
    }
    pub fn list(&mut self, _path: &str) -> Result<Vec<DirectoryEntry>, Error> {
        Err(Error::Protocol)
    }
    pub fn read(&mut self, _path: &str) -> Result<Vec<u8>, Error> {
        Err(Error::Protocol)
    }
    pub fn write(&mut self, _path: &str, _data: &[u8]) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    pub fn rename_replace(&mut self, _from: &str, _to: &str) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    pub fn delete(&mut self, _path: &str) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    pub fn into_parts(self) -> (DirectTcp, Session) {
        (
            DirectTcp,
            Session::new(DialectCap::Default, [0; 16], [0; 32], [0; 3]),
        )
    }
}
