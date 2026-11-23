//! N1570 6.7.1–6.7.3: specifier order is independent of type identity.
use super::*;
use crate::types::{amd64_va_list, Member, RecordDefinition, RecordKind, RecordType};

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
            "extern" | "static" | "typedef" => {
                if self.storage.thread_local && token.is("typedef") {
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
                        _ => Storage::Typedef,
                    };
                    true
                }
            }
            "_Thread_local" => {
                if !allow_storage {
                    return Err(token.error("_Thread_local requires an object declaration"));
                }
                if self.storage.thread_local || self.storage.class == Storage::Typedef {
                    return Err(token.error("conflicting _Thread_local storage specifiers"));
                }
                self.storage.thread_local = true;
                true
            }
            "auto" | "register" if self.storage.thread_local => {
                return Err(token.error("_Thread_local may only be combined with static or extern"));
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
    pub(super) fn specifiers(
        &mut self,
        allow_storage: bool,
    ) -> Result<(Type, DeclStorage), Diagnostic> {
        self.specifiers_at_depth(allow_storage, 0)
            .map(|(ty, storage, _)| (ty, storage))
    }

    pub(super) fn specifiers_at_depth(
        &mut self,
        allow_storage: bool,
        depth: usize,
    ) -> Result<(Type, DeclStorage, bool), Diagnostic> {
        let start = self
            .peek()
            .ok_or_else(|| self.error("expected a type specifier"))?;
        let mut specs = Specifiers::default();
        let mut typedef_signed = false;
        while let Some(token) = self.peek() {
            if self.qualifier(&mut specs.qualifiers) {
                continue;
            }
            if specs.can_take_type_name() {
                let ty = match token.text.as_ref() {
                    "struct" | "union" => {
                        self.take();
                        Some(self.record(token, depth)?)
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
        Ok((
            ty,
            specs.storage,
            specs.unsigned == Some(false) || typedef_signed,
        ))
    }

    fn va_list_type(&mut self) -> Type {
        use amd64_va_list as abi;
        if self.target == Target::Aarch64 {
            // A4 supplies the AAPCS64 representation. Declarations can
            // still refer to an opaque list on that target.
            return Type::new(TypeKind::Record(RecordType {
                id: usize::MAX,
                layout: None,
            }))
            .pointer_to();
        }

        let id = if let Some(id) = self.va_list_record {
            id
        } else {
            let id = self.records.len();
            let unsigned = Type::new(TypeKind::Integer(Integer::UnsignedInt));
            let pointer = Type::new(TypeKind::Void).pointer_to();
            let members = [
                ("gp_offset", unsigned.clone(), abi::GP_OFFSET),
                ("fp_offset", unsigned, abi::FP_OFFSET),
                ("overflow_arg_area", pointer.clone(), abi::OVERFLOW_ARG_AREA),
                ("reg_save_area", pointer, abi::REG_SAVE_AREA),
            ]
            .into_iter()
            .map(|(name, ty, offset)| Member {
                name,
                ty,
                offset,
                bit_field: None,
            })
            .collect();
            self.records.push(Record {
                kind: RecordKind::Struct,
                tag: Some(abi::NAME),
                definition: Some(RecordDefinition {
                    layout: abi::LAYOUT,
                    members,
                    end: None,
                    has_const_member: false,
                }),
            });
            self.va_list_record = Some(id);
            id
        };

        let record = Type::new(TypeKind::Record(RecordType {
            id,
            layout: Some(abi::LAYOUT),
        }));
        Type::new(TypeKind::Array(Box::new(record), 1))
    }

    pub(super) fn qualifier(&mut self, qualifiers: &mut Qualifiers) -> bool {
        if self.eat("const").is_some() {
            qualifiers.constant = true;
            true
        } else if self.eat("volatile").is_some() {
            qualifiers.volatile = true;
            true
        } else {
            self.eat("restrict").is_some()
        }
    }
}
