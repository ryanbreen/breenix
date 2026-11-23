//! N1570 6.7.1–6.7.3: specifier order is independent of type identity.
use super::*;
use crate::types::RecordType;

#[derive(Default)]
struct Specifiers {
    named: Option<Type>,
    base: Option<&'static str>,
    saw_int: bool,
    unsigned: Option<bool>,
    longs: usize,
    qualifiers: Qualifiers,
    storage: Storage,
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
                if !allow_storage || self.storage != Storage::Unspecified {
                    false
                } else {
                    self.storage = match token.text.as_ref() {
                        "extern" => Storage::Extern,
                        "static" => Storage::Static,
                        _ => Storage::Typedef,
                    };
                    true
                }
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
    ) -> Result<(Type, Storage), Diagnostic> {
        self.specifiers_at_depth(allow_storage, 0)
    }

    pub(super) fn specifiers_at_depth(
        &mut self,
        allow_storage: bool,
        depth: usize,
    ) -> Result<(Type, Storage), Diagnostic> {
        let start = self
            .peek()
            .ok_or_else(|| self.error("expected a type specifier"))?;
        let mut specs = Specifiers::default();
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
                    "__builtin_va_list" => {
                        self.take();
                        // Declaration-only representation pending the target va_list
                        // implementation in 25.7b2.
                        Some(
                            Type::new(TypeKind::Record(RecordType {
                                id: usize::MAX,
                                layout: None,
                            }))
                            .pointer_to(),
                        )
                    }
                    name => match self.lookup(name) {
                        Some(Name::Typedef(ty)) => {
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
        let mut ty = Type { kind, qualifiers: specs.qualifiers };
        ty.resolve_records(&|id| self.records.get(id)?.definition.as_ref().map(|def| def.layout));
        Ok((ty, specs.storage))
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
