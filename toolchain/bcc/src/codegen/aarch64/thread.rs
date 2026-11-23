//! AArch64 local-exec TLS (AAELF64 2025Q1 §5.7.11.4).

use super::{check_symbol, Emitter, Linkage, Slot};
use crate::pp::Diagnostic;
use crate::sema::{Object, StaticAddress, ThreadData, ThreadInitializer};
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
        let layout = object.ty.layout().unwrap();
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
        let mut cursor = 0;
        if let Some(initializer) = initializer {
            for (&offset, value) in initializer {
                if offset > cursor {
                    writeln!(data, "    .zero {}", offset - cursor).unwrap();
                }
                let size = match value {
                    ThreadData::Byte(byte) => {
                        writeln!(data, "    .byte {byte}").unwrap();
                        1
                    }
                    ThreadData::Address(address, addend) => {
                        let name = match address {
                            StaticAddress::Symbol(name, site) => {
                                self.use_symbol(name, site)?;
                                (*name).to_owned()
                            }
                            StaticAddress::Local(index) => {
                                let Slot::Static(label) = &self.locals[*index] else {
                                    unreachable!("initializer addresses name ordinary static locals");
                                };
                                label.clone()
                            }
                            StaticAddress::String(bytes) => self.emit_string(bytes),
                        };
                        writeln!(data, "    .quad {name}{addend:+}").unwrap();
                        8
                    }
                };
                cursor = offset + size;
            }
        }
        if cursor < layout.size {
            writeln!(data, "    .zero {}", layout.size - cursor).unwrap();
        }
        writeln!(data, ".size {label}, {}", layout.size).unwrap();
        if initializer.is_some() {
            self.tdata.push_str(&data);
        } else {
            self.tbss.push_str(&data);
        }
        Ok(())
    }
}
