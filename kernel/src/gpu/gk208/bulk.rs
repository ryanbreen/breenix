//! Actual compressed display rows retained until the suite uploads them off-box.
use alloc::{string::String,vec::Vec};
static ROWS:spin::Mutex<Vec<u8>>=spin::Mutex::new(Vec::new());
pub(super) fn line(text:String)->Result<(),&'static str> {
    let mut rows=ROWS.lock();
    if rows.len().saturating_add(text.len()+1)>2<<20 {return Err("display artifact exceeds bound");}
    rows.try_reserve(text.len()+1).map_err(|_|"display artifact storage exhausted")?;
    rows.extend_from_slice(text.as_bytes());rows.push(b'\n');Ok(())
}
pub(super) fn take()->Vec<u8> {core::mem::take(&mut *ROWS.lock())}
