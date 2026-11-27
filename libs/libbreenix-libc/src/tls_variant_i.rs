//! AArch64 variant I static TLS layout, shared by the interpreter and libc.

use crate::tls_module::TlsModule;

#[derive(Clone, Copy)]
pub struct StaticTls {
    pub end: usize,
    pub align: usize,
}

impl StaticTls {
    pub const fn new() -> Self {
        // TPIDR_EL0 points to the two-word TCB, before the first template.
        Self { end: 16, align: 16 }
    }

    pub fn push(&mut self, module: TlsModule) -> Option<usize> {
        if !module.align.is_power_of_two() {
            return None;
        }
        let offset = self.end.checked_add(module.align - 1)? & !(module.align - 1);
        let end = offset.checked_add(module.mem_size)?;
        if end > isize::MAX as usize {
            return None;
        }
        self.end = end;
        self.align = self.align.max(module.align);
        Some(offset)
    }
}
