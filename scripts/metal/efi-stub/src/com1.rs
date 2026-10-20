//! Progress lines on the COM1 UART, which the firmware's console redirection and the remote console share. Each
//! application names itself with a prefix ("metal-stub: ", "breenix-manager: ") that the boot loop reads.
use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

static PREFIX: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
static PREFIX_LEN: AtomicUsize = AtomicUsize::new(0);

/// Sets the prefix the following lines start with.
pub fn set_prefix(prefix: &'static str) {
    PREFIX_LEN.store(prefix.len(), Ordering::Relaxed);
    PREFIX.store(prefix.as_ptr() as *mut u8, Ordering::Relaxed);
}

pub struct Com1;

impl Write for Com1 {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                put(b'\r');
            }
            put(byte);
        }
        Ok(())
    }
}

/// One byte to COM1 once its transmit holding register is empty, or after a bounded wait if no UART answers.
pub fn put(byte: u8) {
    for _ in 0..100_000 {
        let status: u8;
        // SAFETY: COM1's line status register; reading it has no side effect on the transmitter.
        unsafe { asm!("in al, dx", out("al") status, in("dx") 0x3fdu16, options(nomem, nostack)) };
        if status & 0x20 != 0 {
            break;
        }
    }
    // SAFETY: COM1's transmit holding register.
    unsafe { asm!("out dx, al", in("al") byte, in("dx") 0x3f8u16, options(nomem, nostack)) };
}

/// One whole line: the prefix, the text and a line end.
pub fn line(arguments: fmt::Arguments) {
    let prefix = PREFIX.load(Ordering::Relaxed);
    if !prefix.is_null() {
        // SAFETY: set_prefix stored a 'static str's pointer and length.
        let text = unsafe { core::slice::from_raw_parts(prefix, PREFIX_LEN.load(Ordering::Relaxed)) };
        for &byte in text {
            put(byte);
        }
    }
    let _ = Com1.write_fmt(arguments);
    let _ = Com1.write_str("\n");
}

#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {
        $crate::com1::line(format_args!($($arg)*))
    };
}

/// The firmware's vendor string, printable ASCII only.
pub struct Vendor(pub *const u16);

impl fmt::Display for Vendor {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        if self.0.is_null() {
            return out.write_str("(none)");
        }
        for index in 0..64 {
            // SAFETY: the system table's NUL-terminated vendor string, read up to its terminator or 64 units.
            let unit = unsafe { *self.0.add(index) };
            if unit == 0 {
                break;
            }
            let shown = if (0x20..0x7f).contains(&unit) { unit as u8 as char } else { '?' };
            out.write_char(shown)?;
        }
        Ok(())
    }
}

pub fn halt() -> ! {
    loop {
        // SAFETY: masks interrupts and halts the processor for good.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
