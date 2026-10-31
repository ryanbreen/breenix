//! Actual compressed display rows retained until the suite uploads them off-box.
use alloc::{string::String,vec::Vec};
static ROWS:spin::Mutex<Vec<u8>>=spin::Mutex::new(Vec::new());
pub(super) fn line(text:String)->Result<(),&'static str> {
    let mut rows=ROWS.lock();
    if rows.len().saturating_add(text.len()+1)>2<<20 {return Err("display artifact exceeds bound");}
    rows.try_reserve(text.len()+1).map_err(|_|"display artifact storage exhausted")?;
    rows.extend_from_slice(text.as_bytes());rows.push(b'\n');Ok(())
}
pub(super) fn take()->Vec<u8> {
    if let Some((row, anchor)) = super::clock::export() {
        if line(row).is_ok() { crate::serial_println!("{}", anchor); }
        else { crate::serial_println!("GT710_CLOCK_ERROR upload storage exhausted"); }
    }
    for (row, anchor) in super::display_evidence::export() {
        if line(row).is_ok() { crate::serial_println!("{}", anchor); }
        else { crate::serial_println!("GT710_DISPLAY_EVIDENCE_ERROR upload storage exhausted"); }
    }
    match super::display_evidence::export_transitions() {
        Ok(records) => for (row, anchor) in records {
            if line(row).is_ok() { crate::serial_println!("{}", anchor); }
            else { crate::serial_println!("GT710_DISPLAY_EVIDENCE_ERROR upload storage exhausted"); }
        },
        Err(reason) => crate::serial_println!("GT710_DISPLAY_EVIDENCE_ERROR {}", reason),
    }
    if let Some((text, anchor)) = super::timing::export() {
        // A transport overflow leaves a missing record, which the decoder rejects.
        let _ = line(text);
        crate::serial_println!("{}", anchor);
    }
    core::mem::take(&mut *ROWS.lock())
}

pub(super) fn used() -> usize { ROWS.lock().len() }

#[cfg(target_os="none")]
pub(super) fn capacity() -> usize { ROWS.lock().capacity() }
