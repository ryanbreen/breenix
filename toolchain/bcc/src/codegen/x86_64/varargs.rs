//! System V AMD64 variable argument lists, psABI §§3.2.3 and 3.5.7.

use super::abi::{Class, INTEGER_REGS, VECTOR_ARG_REGS};
use super::*;
use crate::types::amd64_va_list::{FP_OFFSET, GP_OFFSET, LAYOUT, OVERFLOW_ARG_AREA, REG_SAVE_AREA};

const GP_STRIDE: usize = 8;
const FP_STRIDE: usize = 16;
const GP_LIMIT: usize = INTEGER_REGS.len() * GP_STRIDE;
// §3.2.3 assigns only XMM0–7 to arguments. No argument can occupy the
// XMM8–15 entries pictured in the pinned §3.5.7 register-save diagram.
const FP_LIMIT: usize = GP_LIMIT + VECTOR_ARG_REGS * FP_STRIDE;

pub(super) struct VariadicFrame {
    pub(super) save: Slot,
    pub(super) named: CallLayout,
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn save_variadic_registers(&mut self, site: &Position) -> Result<Slot, Diagnostic> {
        let save = self.frame_slot(
            Layout {
                size: FP_LIMIT,
                align: FP_STRIDE,
            },
            site,
        )?;
        for (index, reg) in INTEGER_REGS.iter().enumerate() {
            writeln!(
                self.text,
                "    mov qword {}, {reg}",
                save.memory(index * GP_STRIDE)
            )
            .unwrap();
        }
        let done = self.label();
        writeln!(self.text, "    test al, al\n    jz {done}").unwrap();
        for index in 0..VECTOR_ARG_REGS {
            let dest = save.memory(GP_LIMIT + index * FP_STRIDE);
            writeln!(self.text, "    movaps {dest}, xmm{index}").unwrap();
        }
        writeln!(self.text, "{done}:").unwrap();
        Ok(save)
    }

    pub(super) fn va_start(&mut self, list: &Expression<'src>) -> Result<(), Diagnostic> {
        self.value(list)?;
        let frame = self.variadic.as_ref().unwrap();
        let gp = frame.named.integers * GP_STRIDE;
        let fp = GP_LIMIT + frame.named.vectors * FP_STRIDE;
        let overflow = 16 + frame.named.stack;
        let save = frame.save.memory(0);
        writeln!(self.text, "    mov dword [rax + {GP_OFFSET}], {gp}").unwrap();
        writeln!(self.text, "    mov dword [rax + {FP_OFFSET}], {fp}").unwrap();
        writeln!(
            self.text,
            "    lea r10, [rbp + {overflow}]\n    mov qword [rax + {OVERFLOW_ARG_AREA}], r10"
        )
        .unwrap();
        writeln!(
            self.text,
            "    lea r10, {save}\n    mov qword [rax + {REG_SAVE_AREA}], r10"
        )
        .unwrap();
        Ok(())
    }

    pub(super) fn va_copy(
        &mut self,
        dest: &Expression<'src>,
        source: &Expression<'src>,
    ) -> Result<(), Diagnostic> {
        self.value(dest)?;
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        self.value(source)?;
        self.text
            .push_str("    mov rsi, rax\n    mov rdi, qword [rsp]\n");
        self.adjust_stack("add", 16);
        self.copy_record(LAYOUT.size);
        Ok(())
    }

    pub(super) fn va_arg(
        &mut self,
        list: &Expression<'src>,
        ty: &Type,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        self.value(list)?;
        self.text.push_str("    mov r11, rax\n");

        let layout = ty.layout().unwrap();
        let classes = match self.record_abi(ty) {
            Some(RecordAbi::Memory) => None,
            Some(RecordAbi::Registers(classes))
                if classes
                    .iter()
                    .all(|class| matches!(class, Class::Integer | Class::Sse | Class::Empty)) =>
            {
                Some(classes)
            }
            Some(RecordAbi::Registers(_)) => None,
            None if matches!(ty.kind, Kind::Float(_)) => Some([Class::Sse, Class::Empty]),
            None if ty.kind == Kind::LongDouble => None,
            None => Some([Class::Integer, Class::Empty]),
        };
        let done = if let Some(classes) = classes {
            let overflow = self.label();
            let done = self.label();
            for (class, offset, limit, stride) in [
                (Class::Integer, GP_OFFSET, GP_LIMIT, GP_STRIDE),
                (Class::Sse, FP_OFFSET, FP_LIMIT, FP_STRIDE),
            ] {
                let count = classes.iter().filter(|&&piece| piece == class).count();
                if count != 0 {
                    let last = limit - count * stride;
                    writeln!(
                        self.text,
                        "    cmp dword [r11 + {offset}], {last}\n    ja {overflow}"
                    )
                    .unwrap();
                }
            }
            // Check both banks before updating either. SSE save slots are
            // sixteen bytes apart, so gather register records into C layout.
            if matches!(ty.kind, Kind::Record(_)) {
                let size = abi::eightbyte_size(layout.size, site)?;
                let slot = self.frame_slot(
                    Layout {
                        size,
                        align: layout.align,
                    },
                    site,
                )?;
                for (index, class) in classes.into_iter().enumerate() {
                    if self.va_register_address(class) {
                        let dest = slot.memory(index * 8);
                        writeln!(
                            self.text,
                            "    mov r10, qword [rax]\n    mov qword {dest}, r10"
                        )
                        .unwrap();
                    }
                }
                writeln!(self.text, "    lea rax, {}", slot.memory(0)).unwrap();
            } else {
                self.va_register_address(classes[0]);
            }
            writeln!(self.text, "    jmp {done}\n{overflow}:").unwrap();
            Some(done)
        } else {
            None
        };

        writeln!(self.text, "    mov rax, qword [r11 + {OVERFLOW_ARG_AREA}]").unwrap();
        if layout.align > 8 {
            let alignment = layout.align.max(16);
            writeln!(
                self.text,
                "    add rax, {}
    and rax, -{alignment}",
                alignment - 1
            )
            .unwrap();
        }
        let step = abi::checked_stack_end(0, abi::eightbyte_size(layout.size, site)?, site)?;
        writeln!(
            self.text,
            "    lea r10, [rax + {step}]\n    mov qword [r11 + {OVERFLOW_ARG_AREA}], r10"
        )
        .unwrap();
        if let Some(done) = done {
            writeln!(self.text, "{done}:").unwrap();
        }
        // Scalar results are loaded at their declared width. A record value
        // is represented by its address, as with a record-returning call.
        self.load(ty, "[rax]");
        Ok(())
    }

    fn va_register_address(&mut self, class: Class) -> bool {
        let (offset, stride) = match class {
            Class::Integer => (GP_OFFSET, GP_STRIDE),
            Class::Sse => (FP_OFFSET, FP_STRIDE),
            Class::Empty => return false,
            Class::X87 | Class::X87Up => unreachable!("x87 arguments use the overflow area"),
        };
        writeln!(self.text, "    mov ecx, dword [r11 + {offset}]").unwrap();
        writeln!(
            self.text,
            "    mov rax, qword [r11 + {REG_SAVE_AREA}]\n    add rax, rcx"
        )
        .unwrap();
        writeln!(self.text, "    add dword [r11 + {offset}], {stride}").unwrap();
        true
    }
}
