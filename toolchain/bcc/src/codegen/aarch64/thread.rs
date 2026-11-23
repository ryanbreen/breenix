//! AArch64 local-exec TLS (AAELF64 2025Q1 §5.7.11.4).

use super::{check_symbol, Emitter, Linkage};
use crate::pp::Diagnostic;
use crate::sema::{Object, ThreadInitializer};
use std::fmt::Write;

pub(super) fn thread_address(out: &mut String, register: &str, name: &str) {
    // In bas's dialect, :tprel_hi12: selects the ADD shifted by 12 bits.
    writeln!(
        out,
        "    mrs {register}, tpidr_el0\n    add {register}, {register}, :tprel_hi12:{name}\n    add {register}, {register}, :tprel_lo12_nc:{name}"
    )
    .unwrap();
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn thread_definition(
        &mut self,
        object: &Object<'src>,
        label: &str,
        linkage: Linkage,
        initializer: Option<&ThreadInitializer<'src>>,
    ) -> Result<(), Diagnostic> {
        check_symbol(label, object.site)?;
        let layout = object.layout().unwrap();
        let mut data = String::new();
        if linkage == Linkage::External {
            writeln!(data, ".global {label}").unwrap();
        }
        writeln!(
            data,
            ".type {label}, @tls\n    .balign {}\n{label}:",
            layout.align
        )
        .unwrap();
        self.emit_initializer(&mut data, initializer, layout.size)?;
        writeln!(data, ".size {label}, {}", layout.size).unwrap();
        if initializer.is_some() {
            self.tdata.push_str(&data);
        } else {
            self.tbss.push_str(&data);
        }
        Ok(())
    }
}
