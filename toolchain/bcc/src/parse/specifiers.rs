//! N1570 6.7.1–6.7.3: specifier order is independent of type identity.
use super::*;
use crate::types::{aarch64_va_list, amd64_va_list, Member, RecordDefinition, RecordKind, RecordType};

#[derive(Default)]
struct Specifiers {
    named: Option<Type>,
    base: Option<&'static str>,
    saw_int: bool,
    unsigned: Option<bool>,
    longs: usize,
    qualifiers: Qualifiers,
    storage: DeclStorage,
}

impl Specifiers {
    fn push(&mut self, token: &Lexeme, allow_storage: bool) -> Result<bool, Diagnostic> {
        let valid = match token.text.as_ref() {
            "long" => {
                self.longs += 1;
                self.longs <= 2
            }
            "signed" | "unsigned" => self.unsigned.replace(token.is("unsigned")).is_none(),
            "extern" | "static" | "typedef" | "auto" | "register" => {
                if self.storage.thread_local && !matches!(token.text.as_ref(), "extern" | "static")
                {
                    return Err(
                        token.error("_Thread_local may only be combined with static or extern")
                    );
                }
                if !allow_storage || self.storage.class != Storage::Unspecified {
                    false
                } else {
                    self.storage.class = match token.text.as_ref() {
                        "extern" => Storage::Extern,
                        "static" => Storage::Static,
                        "typedef" => Storage::Typedef,
                        "auto" => Storage::Auto,
                        _ => Storage::Register,
                    };
                    true
                }
            }
            "_Thread_local" => {
                if !allow_storage {
                    return Err(token.error("_Thread_local requires an object declaration"));
                }
                if self.storage.thread_local
                    || !matches!(
                        self.storage.class,
                        Storage::Unspecified | Storage::Extern | Storage::Static
                    )
                {
                    return Err(token.error("conflicting _Thread_local storage specifiers"));
                }
                self.storage.thread_local = true;
                true
            }
            "void" => self.base.replace("void").is_none(),
            "float" => self.base.replace("float").is_none(),
            "double" => self.base.replace("double").is_none(),
            "_Bool" => self.base.replace("_Bool").is_none(),
            "char" => self.base.replace("char").is_none(),
            "short" => self.base.replace("short").is_none(),
            "int" => !std::mem::replace(&mut self.saw_int, true),
            _ => return Ok(false),
        };
        if !valid {
            return Err(token.error("conflicting declaration specifiers"));
        }
        Ok(true)
    }

    fn can_take_type_name(&self) -> bool {
        self.named.is_none()
            && self.base.is_none()
            && !self.saw_int
            && self.unsigned.is_none()
            && self.longs == 0
    }

    fn kind(&self) -> Option<TypeKind> {
        if let Some(named) = &self.named {
            if self.base.is_some() || self.saw_int || self.unsigned.is_some() || self.longs != 0 {
                return None;
            }
            return Some(named.kind.clone());
        }
        use Integer::*;
        if self.saw_int && !matches!(self.base, None | Some("short")) {
            return None;
        }
        let base = self.base.or(self.saw_int.then_some("int"));
        let unsigned = self.unsigned == Some(true);
        let integer = match (base, self.longs) {
            (Some("float"), 0) if self.unsigned.is_none() => {
                return Some(TypeKind::Float(crate::types::Float::Single));
            }
            (Some("double"), 1) if self.unsigned.is_none() => return Some(TypeKind::LongDouble),
            (Some("double"), 0) if self.unsigned.is_none() => {
                return Some(TypeKind::Float(crate::types::Float::Double));
            }
            (Some("void"), 0) if self.unsigned.is_none() => return Some(TypeKind::Void),
            (Some("_Bool"), 0) if self.unsigned.is_none() => Bool,
            (Some("char"), 0) => match self.unsigned {
                None => Char,
                Some(false) => SignedChar,
                Some(true) => UnsignedChar,
            },
            (Some("short"), 0) => {
                if unsigned {
                    UnsignedShort
                } else {
                    Short
                }
            }
            (Some("int"), 0) => {
                if unsigned {
                    UnsignedInt
                } else {
                    Int
                }
            }
            (None, 0) if self.unsigned.is_some() => {
                if unsigned {
                    UnsignedInt
                } else {
                    Int
                }
            }
            (None | Some("int"), 1) => {
                if unsigned {
                    UnsignedLong
                } else {
                    Long
                }
            }
            (None | Some("int"), 2) => {
                if unsigned {
                    UnsignedLongLong
                } else {
                    LongLong
                }
            }
            _ => return None,
        };
        Some(TypeKind::Integer(integer))
    }
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    pub(super) fn specifiers_at_depth(
        &mut self,
        allow_storage: bool,
        depth: usize,
    ) -> Result<(Type, DeclStorage, bool), Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(self.error("type specifier nesting limit exceeded"));
        }
        let start = self
            .peek()
            .ok_or_else(|| self.error("expected a type specifier"))?;
        let mut specs = Specifiers::default();
        let mut typedef_signed = false;
        while let Some(token) = self.peek() {
            // N1570 6.7.2p3 and 6.10.8.3 exclude complex types from this profile.
            if token.is("_Complex") {
                return Err(token.error("_Complex is not supported by bcc"));
            }
            // N1570 6.7.2.4p4: a following '(' selects the specifier.
            if token.is("_Atomic") {
                self.take();
                if self.eat("(").is_some() {
                    if !specs.can_take_type_name() {
                        return Err(token.error("conflicting declaration specifiers"));
                    }
                    let mut ty = self.type_name(depth + 1)?;
                    self.expect(")", "expected ')' after the atomic type name")?;
                    if ty.qualifiers != Qualifiers::default() {
                        return Err(token.error("_Atomic type specifier requires an unqualified type"));
                    }
                    ty.qualifiers.atomic = true;
                    ty.check_atomic(token)?;
                    specs.named = Some(ty);
                }
                specs.qualifiers.atomic = true;
                continue;
            }
            if self.eat("__extension__").is_some() || self.qualifier(&mut specs.qualifiers) {
                continue;
            }
            if matches!(token.text.as_ref(), "__attribute__" | "__attribute") {
                for (site, attribute) in self.attributes(depth)? {
                    specs.storage.apply_attribute(site, attribute)?;
                }
                continue;
            }
            if token.is("_Alignas") {
                self.take();
                let alignment = self.alignment_specifier(token, depth + 1)?;
                specs.storage.alignment = Some(specs.storage.alignment.unwrap_or(0).max(alignment));
                continue;
            }
            if token.is("_Noreturn") {
                self.take();
                if !allow_storage {
                    return Err(token.error("_Noreturn requires a function declaration"));
                }
                specs.storage.noreturn = true;
                continue;
            }
            if matches!(token.text.as_ref(), "inline" | "__inline" | "__inline__") {
                self.take();
                if !allow_storage {
                    return Err(token.error("inline requires a function declaration"));
                }
                specs.storage.inline = true;
                continue;
            }
            if specs.can_take_type_name() {
                let ty = match token.text.as_ref() {
                    "struct" | "union" => {
                        self.take();
                        let ty = self.record(token, depth)?;
                        let TypeKind::Record(record) = ty.kind else {
                            unreachable!()
                        };
                        specs.storage.anonymous_record = self.records[record.id].tag.is_none();
                        Some(ty)
                    }
                    _ if self.is_typeof(token) => {
                        self.take();
                        self.expect("(", "expected '(' after typeof")?;
                        let ty = if self.peek().is_some_and(|token| self.starts_type(token)) {
                            self.type_name(depth + 1)?
                        } else {
                            let expression = self.expression(depth + 1)?;
                            crate::sema::typeof_expression(expression, self.expression_scope())?
                        };
                        self.expect(")", "expected ')' after the typeof operand")?;
                        Some(ty)
                    }
                    "enum" => {
                        self.take();
                        Some(self.enumeration(token)?)
                    }
                    amd64_va_list::NAME => {
                        self.take();
                        Some(self.va_list_type())
                    }
                    name => match self.lookup(name) {
                        Some(Name::Typedef(ty, signed)) => {
                            typedef_signed = *signed;
                            let ty = ty.clone();
                            self.take();
                            Some(ty)
                        }
                        _ => None,
                    },
                };
                if let Some(ty) = ty {
                    specs.qualifiers.constant |= ty.qualifiers.constant;
                    specs.qualifiers.volatile |= ty.qualifiers.volatile;
                    specs.qualifiers.atomic |= ty.qualifiers.atomic;
                    specs.qualifiers.restricted |= ty.qualifiers.restricted;
                    specs.qualifiers.may_alias |= ty.qualifiers.may_alias;
                    specs.named = Some(ty);
                    continue;
                }
            }
            if !specs.push(token, allow_storage)? {
                break;
            }
            self.take();
        }
        let kind = specs.kind().ok_or_else(|| {
            let message = "expected a valid type specifier";
            if self.peek().is_none() {
                self.error(message)
            } else {
                start.error(message)
            }
        })?;
        let mut ty = Type {
            kind,
            qualifiers: specs.qualifiers,
        };
        ty.resolve_records(&|id| {
            self.records
                .get(id)?
                .definition
                .as_ref()
                .map(|def| def.layout)
        });
        ty.check_atomic(start)?;
        Ok((
            ty,
            specs.storage,
            specs.unsigned == Some(false) || typedef_signed,
        ))
    }

    fn va_list_type(&mut self) -> Type {
        use aarch64_va_list as arm;
        use amd64_va_list as x86;
        let (name, layout) = match self.target {
            Target::Aarch64 => (arm::NAME, arm::LAYOUT),
            Target::X86_64 => (x86::NAME, x86::LAYOUT),
        };

        let id = if let Some(id) = self.va_list_record {
            id
        } else {
            let id = self.records.len();
            let pointer = Type::new(TypeKind::Void).pointer_to();
            let fields = match self.target {
                Target::Aarch64 => {
                    let int = Type::new(TypeKind::Integer(Integer::Int));
                    vec![
                        ("__stack", pointer.clone(), arm::STACK),
                        ("__gr_top", pointer.clone(), arm::GR_TOP),
                        ("__vr_top", pointer, arm::VR_TOP),
                        ("__gr_offs", int.clone(), arm::GR_OFFS),
                        ("__vr_offs", int, arm::VR_OFFS),
                    ]
                }
                Target::X86_64 => {
                    let unsigned = Type::new(TypeKind::Integer(Integer::UnsignedInt));
                    vec![
                        ("gp_offset", unsigned.clone(), x86::GP_OFFSET),
                        ("fp_offset", unsigned, x86::FP_OFFSET),
                        ("overflow_arg_area", pointer.clone(), x86::OVERFLOW_ARG_AREA),
                        ("reg_save_area", pointer, x86::REG_SAVE_AREA),
                    ]
                }
            };
            let members = fields
            .into_iter()
            .map(|(name, ty, offset)| Member {
                name: Some(name),
                ty,
                offset,
                bit_field: None,
            })
            .collect();
            self.records.push(Record {
                kind: RecordKind::Struct,
                tag: Some(name),
                definition: Some(RecordDefinition {
                    layout,
                    members,
                    end: None,
                    has_const_member: false,
                    may_alias: false,
                }),
            });
            self.va_list_record = Some(id);
            id
        };

        let record = Type::new(TypeKind::Record(RecordType {
            id,
            layout: Some(layout),
        }));
        match self.target {
            Target::Aarch64 => record,
            Target::X86_64 => Type::new(TypeKind::Array(Box::new(record), Some(1))),
        }
    }

    pub(super) fn qualifier(&mut self, qualifiers: &mut Qualifiers) -> bool {
        if self.eat("const").is_some() {
            qualifiers.constant = true;
            true
        } else if self.eat("volatile").is_some() {
            qualifiers.volatile = true;
            true
        } else if self
            .peek()
            .is_some_and(|token| matches!(token.text.as_ref(), "restrict" | "__restrict" | "__restrict__"))
        {
            self.take();
            qualifiers.restricted = true;
            true
        } else {
            false
        }
    }
}
