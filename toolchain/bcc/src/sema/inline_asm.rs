//! Register constraints from GNU C §§6.11.2–6.11.3.

use super::*;

#[derive(Clone, Debug)]
pub struct InlineAsm<'a> {
    pub template: String,
    pub registers: Vec<AsmRegister<'a>>,
}

#[derive(Clone, Debug)]
pub struct AsmRegister<'a> {
    // Each occupied register has a private 64-bit staging slot. The compiler evaluates calls
    // and output address expressions outside the assembly block.
    pub slot: Expression<'a>,
    pub input: bool,
    pub output: bool,
}

const AMD64_REGISTERS: [[&str; 4]; 6] = [
    ["r10b", "r10w", "r10d", "r10"],
    ["r11b", "r11w", "r11d", "r11"],
    ["cl", "cx", "ecx", "rcx"],
    ["dl", "dx", "edx", "rdx"],
    ["sil", "si", "esi", "rsi"],
    ["dil", "di", "edi", "rdi"],
];
const A64_REGISTERS: [[&str; 2]; 6] = [
    ["w10", "x10"],
    ["w11", "x11"],
    ["w12", "x12"],
    ["w13", "x13"],
    ["w14", "x14"],
    ["w15", "x15"],
];

// Use call-clobbered registers outside the expression result, address scratch
// and AMD64 linear-scan register banks (AMD64 psABI §3.2.1, AAPCS64 §6.1.1).
pub(crate) fn asm_register(target: Target, index: usize, width: usize) -> &'static str {
    match target {
        Target::X86_64 => AMD64_REGISTERS[index][width.trailing_zeros() as usize],
        Target::Aarch64 => A64_REGISTERS[index][usize::from(width == 8)],
    }
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn inline_asm(
        &mut self,
        asm: parse::inline_asm::InlineAsm<'a>,
    ) -> Result<Statement<'a>, Diagnostic> {
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut registers = Vec::new();
        let mut operands = Vec::new();
        let word = Type::new(Kind::Integer(Integer::UnsignedLong));

        for operand in asm.outputs {
            let read = match operand.constraint.as_str() {
                "=r" => false,
                "+r" => true,
                constraint => {
                    return Err(operand
                        .site
                        .error(format!("unsupported asm output constraint '{constraint}'")))
                }
            };
            let output = self.expression(operand.value)?;
            if !output.modifiable(self.records.records)
                || output.bit_field().is_some()
                || output.ty.qualifiers.atomic
            {
                return Err(output
                    .site
                    .error("asm output requires an addressable non-atomic lvalue"));
            }
            let width = register_width(&output.ty, output.site)?;
            let pointer_type = output.ty.clone().pointer_to();
            let place = self.asm_local(asm.site, &pointer_type);
            let value = Expression {
                site: output.site,
                ty: output.ty.clone(),
                kind: ExpressionKind::Unary(Unary::Dereference, Box::new(place.clone())),
            };
            let address = Expression {
                site: output.site,
                ty: pointer_type,
                kind: ExpressionKind::Unary(Unary::Address, Box::new(output)),
            };
            before.push(assign(place, address));

            let slot = self.asm_local(asm.site, &word);
            if read {
                before.push(assign(slot.clone(), cast(value.clone(), &word)));
            }
            after.push(assign(
                value.clone(),
                cast(slot.clone(), &value.ty.unqualified()),
            ));
            operands.push((registers.len(), width));
            registers.push(AsmRegister {
                slot,
                input: read,
                output: true,
            });
        }

        let output_count = registers.len();
        for operand in asm.inputs {
            let value = self.expression(operand.value)?;
            let width = register_width(&value.value_type(), value.site)?;
            let index = if operand.constraint == "r" {
                let index = registers.len();
                registers.push(AsmRegister {
                    slot: self.asm_local(asm.site, &word),
                    input: false,
                    output: false,
                });
                index
            } else {
                let index = operand
                    .constraint
                    .parse::<usize>()
                    .ok()
                    .filter(|&index| index < output_count)
                    .ok_or_else(|| {
                        operand.site.error(format!(
                            "unsupported asm input constraint '{}'",
                            operand.constraint
                        ))
                    })?;
                if registers[index].input || width != operands[index].1 {
                    return Err(operand.site.error(
                        "asm matching input must supply one equally wide write-only output",
                    ));
                }
                index
            };
            let register = &mut registers[index];
            register.input = true;
            before.push(assign(register.slot.clone(), cast(value, &word)));
            operands.push((index, width));
        }

        if registers.len() > AMD64_REGISTERS.len() {
            return Err(asm.site.error("too many independent asm register operands"));
        }
        let template = if asm.extended {
            substitute(&asm.template, self.target, &operands, asm.site)?
        } else {
            asm.template
        };
        before.push(Statement::InlineAsm(InlineAsm {
            template,
            registers,
        }));
        before.extend(after);
        Ok(Statement::Block(before))
    }

    fn asm_local(&mut self, site: &'a Lexeme, ty: &Type) -> Expression<'a> {
        Expression {
            site,
            ty: ty.clone(),
            kind: ExpressionKind::Local(self.temporary(site, ty)),
        }
    }
}

fn register_width(ty: &Type, site: &Lexeme) -> Result<usize, Diagnostic> {
    match ty.kind {
        Kind::Integer(integer) => Ok(integer.layout().size),
        Kind::Pointer(_) => Ok(8),
        _ => Err(site.error("asm register operand requires an integer or pointer")),
    }
}

fn cast<'a>(value: Expression<'a>, ty: &Type) -> Expression<'a> {
    Expression {
        site: value.site,
        ty: ty.clone(),
        kind: ExpressionKind::Cast(Box::new(value)),
    }
}

fn assign<'a>(left: Expression<'a>, right: Expression<'a>) -> Statement<'a> {
    Statement::Expression(Expression {
        site: left.site,
        ty: left.ty.unqualified(),
        kind: ExpressionKind::Assignment {
            left: Box::new(left),
            right: Box::new(right),
            operation: None,
        },
    })
}

fn substitute(
    template: &str,
    target: Target,
    operands: &[(usize, usize)],
    site: &Lexeme,
) -> Result<String, Diagnostic> {
    let mut text = String::new();
    let mut parts = template.split('%');
    text.push_str(parts.next().unwrap_or_default());
    let mut parts = parts.peekable();
    while let Some(mut operand) = parts.next() {
        if operand.is_empty() && parts.peek().is_some() {
            text.push('%');
            text.push_str(parts.next().unwrap());
            continue;
        }
        let explicit_width = match (target, operand.as_bytes().first()) {
            (Target::X86_64, Some(b'k')) | (Target::Aarch64, Some(b'w')) => Some(4),
            (Target::X86_64, Some(b'q')) | (Target::Aarch64, Some(b'x')) => Some(8),
            _ => None,
        };
        if explicit_width.is_some() {
            operand = &operand[1..];
        }
        let after = operand.bytes().take_while(u8::is_ascii_digit).count();
        let (register, width) = operand[..after]
            .parse::<usize>()
            .ok()
            .and_then(|index| operands.get(index).copied())
            .ok_or_else(|| {
                site.error("unsupported asm template operand; expected a register index")
            })?;
        text.push_str(asm_register(
            target,
            register,
            explicit_width.unwrap_or(width),
        ));
        text.push_str(&operand[after..]);
    }
    Ok(text)
}
