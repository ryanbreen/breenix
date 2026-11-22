//! N1570 6.7.1–6.7.3: specifier order is independent of type identity.
use super::*;

#[derive(Default)]
struct Specifiers {
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
            "extern" | "static" => {
                if !allow_storage || self.storage != Storage::Unspecified {
                    false
                } else {
                    self.storage = if token.is("extern") {
                        Storage::Extern
                    } else {
                        Storage::Static
                    };
                    true
                }
            }
            "void" => self.base.replace("void").is_none(),
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

    fn kind(&self) -> Option<TypeKind> {
        use Integer::*;
        if self.saw_int && !matches!(self.base, None | Some("short")) {
            return None;
        }
        let base = self.base.or(self.saw_int.then_some("int"));
        let unsigned = self.unsigned == Some(true);
        let integer = match (base, self.longs) {
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
        let start = self
            .peek()
            .ok_or_else(|| self.error("expected a type specifier"))?;
        let mut specs = Specifiers::default();
        while let Some(token) = self.peek() {
            if self.qualifier(&mut specs.qualifiers) {
                continue;
            }
            if !specs.push(token, allow_storage)? {
                break;
            }
            self.take();
        }
        let kind = specs.kind().ok_or_else(|| {
            let message = "expected a valid integer or void type";
            if self.peek().is_none() {
                self.error(message)
            } else {
                start.error(message)
            }
        })?;
        Ok((
            Type {
                kind,
                qualifiers: specs.qualifiers,
            },
            specs.storage,
        ))
    }

    pub(super) fn qualifier(&mut self, qualifiers: &mut Qualifiers) -> bool {
        if self.eat("const").is_some() {
            qualifiers.constant = true;
            true
        } else if self.eat("volatile").is_some() {
            qualifiers.volatile = true;
            true
        } else {
            false
        }
    }
}
