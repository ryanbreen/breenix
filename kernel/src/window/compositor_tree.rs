//! Access and storage for a compositor claim's semantic tree.

use breengel_abi::errno::EPERM;

#[derive(Debug, PartialEq, Eq)]
pub enum Access { Read, Write }

pub fn access(holder: Option<u64>, caller: u64, root: bool) -> Result<Access, i32> {
    match holder {
        Some(id) if id == caller => Ok(Access::Write),
        Some(_) if root => Ok(Access::Read),
        _ => Err(EPERM),
    }
}

pub struct ClaimTree<T> {
    pub holder: u64,
    pub page: T,
    pub readers: T,
}

impl<T> ClaimTree<T> {
    pub fn new(holder: u64, page: T, readers: T) -> Self {
        Self { holder, page, readers }
    }
}
