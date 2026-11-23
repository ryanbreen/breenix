//! AMD64 psABI §10.3.2 local-exec TLS and ELF gABI §§3.10, 5.2 and 7.7.

use super::*;

pub(super) struct ThreadPlace {
    symbol: String,
    offset: usize,
}

impl ThreadPlace {
    pub(super) fn memory(&self, byte: usize) -> String {
        format!("fs:[{}]", self.displacement(byte))
    }

    pub(super) fn address_operand(&self) -> String {
        format!("[rax + {}]", self.displacement(0))
    }

    fn displacement(&self, byte: usize) -> String {
        let offset = self.offset + byte;
        if offset == 0 {
            format!("{}@tpoff", self.symbol)
        } else {
            format!("{}@tpoff + {offset}", self.symbol)
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn thread_scalar_place(
        &mut self,
        expr: &Expression<'src>,
    ) -> Result<Option<ThreadPlace>, Diagnostic> {
        if matches!(expr.ty.kind, Kind::Array(..) | Kind::Record(_)) {
            return Ok(None);
        }
        self.thread_place(expr)
    }

    pub(super) fn thread_place(
        &mut self,
        expr: &Expression<'src>,
    ) -> Result<Option<ThreadPlace>, Diagnostic> {
        let symbol = match &expr.kind {
            ExpressionKind::ThreadLocal(name) => {
                self.check_symbol(name, expr.position())?;
                if !self.thread_definitions.contains(name) {
                    self.thread_externs.insert(name);
                }
                (*name).to_owned()
            }
            ExpressionKind::Local(index) => match &self.locals[*index] {
                Slot::Thread(label) => label.clone(),
                _ => return Ok(None),
            },
            ExpressionKind::Member { base, offset, .. } => {
                return Ok(self.thread_place(base)?.map(|mut place| {
                    place.offset += offset;
                    place
                }));
            }
            _ => return Ok(None),
        };
        Ok(Some(ThreadPlace { symbol, offset: 0 }))
    }

    pub(super) fn thread_definition(
        &mut self,
        object: &Object<'src>,
        label: &str,
        linkage: Linkage,
        initializer: Option<&ThreadInitializer<'src>>,
    ) -> Result<(), Diagnostic> {
        self.check_symbol(label, object.site)?;
        let layout = object.ty.layout().unwrap();
        // AMD64 psABI §3.1.2 gives array variables of at least 16 bytes
        // extra alignment independently of their element type.
        let align = if matches!(object.ty.kind, Kind::Array(..)) && layout.size >= 16 {
            layout.align.max(16)
        } else {
            layout.align
        };
        let mut data = String::new();
        if linkage == Linkage::External {
            writeln!(data, "global {label}").unwrap();
        }
        writeln!(data, ".type {label}, @tls\n    .align {align}\n{label}:").unwrap();
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
                                    unreachable!(
                                        "an initializer address names an ordinary static local"
                                    );
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
