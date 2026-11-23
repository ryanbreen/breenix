//! Declaration attributes from the pinned GNU C Common Attributes manual.

use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SymbolAttributes {
    pub section: Option<String>,
    pub visibility: Option<&'static str>,
    pub alias: Option<String>,
    pub weak: bool,
}

impl SymbolAttributes {
    pub(crate) fn merge(&mut self, other: Self, site: &Lexeme) -> Result<(), Diagnostic> {
        fn merge_value<T: Eq>(old: &mut Option<T>, new: Option<T>) -> bool {
            match (old.as_ref(), new) {
                (Some(old), Some(new)) => *old == new,
                (None, new) => {
                    *old = new;
                    true
                }
                (_, None) => true,
            }
        }
        if !merge_value(&mut self.section, other.section)
            || !merge_value(&mut self.visibility, other.visibility)
            || !merge_value(&mut self.alias, other.alias)
        {
            return Err(site.error("conflicting GNU symbol attributes"));
        }
        self.weak |= other.weak;
        Ok(())
    }
}

#[derive(Clone)]
pub(super) enum Attribute {
    Aligned(usize),
    Packed,
    Noreturn,
    ReturnsTwice,
    VectorSize(usize),
    MayAlias,
    Symbol(SymbolAttributes),
    FunctionHint(&'static str),
    Unused,
    Used,
    Deprecated,
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    pub(super) fn attributes(
        &mut self,
        depth: usize,
    ) -> Result<Vec<(&'a Lexeme, Attribute)>, Diagnostic> {
        let mut attributes = Vec::new();
        while self
            .eat("__attribute__")
            .or_else(|| self.eat("__attribute"))
            .is_some()
        {
            self.expect("(", "expected '(' after __attribute__")?;
            self.expect("(", "expected '(' before the attribute list")?;
            while self.eat(")").is_none() {
                if self.eat(",").is_some() {
                    continue;
                }
                let token = self
                    .take()
                    .ok_or_else(|| self.error("expected an attribute name"))?;
                let name = token
                    .text
                    .strip_prefix("__")
                    .and_then(|name| name.strip_suffix("__"))
                    .unwrap_or(&token.text);
                let attribute = match name {
                    "aligned" => {
                        let alignment = if self.eat("(").is_some() {
                            let expression = self.conditional(depth + 1)?;
                            let value = self.layout_constant(expression)?;
                            self.expect(")", "expected ')' after the alignment")?;
                            if !value.is_power_of_two() || value > 4096 {
                                return Err(
                                    token.error("alignment must be a power of two up to 4096")
                                );
                            }
                            value as usize
                        } else {
                            // Both Breenix LP64 targets have 16-byte fundamental
                            // alignment for long double.
                            16
                        };
                        Attribute::Aligned(alignment)
                    }
                    "packed" => Attribute::Packed,
                    "noreturn" => Attribute::Noreturn,
                    "returns_twice" => Attribute::ReturnsTwice,
                    "may_alias" => Attribute::MayAlias,
                    "unused" => Attribute::Unused,
                    "used" => Attribute::Used,
                    "weak" => Attribute::Symbol(SymbolAttributes {
                        weak: true,
                        ..Default::default()
                    }),
                    "section" => {
                        let section = self.attribute_string(token)?;
                        if section.is_empty() || section.contains('\0') {
                            return Err(token.error("section requires a nonempty name without NUL"));
                        }
                        Attribute::Symbol(SymbolAttributes {
                            section: Some(section),
                            ..Default::default()
                        })
                    }
                    "visibility" => {
                        let visibility = match self.attribute_string(token)?.as_str() {
                            "default" => "default",
                            "internal" => "internal",
                            "hidden" => "hidden",
                            "protected" => "protected",
                            _ => return Err(token.error("invalid ELF symbol visibility")),
                        };
                        Attribute::Symbol(SymbolAttributes {
                            visibility: Some(visibility),
                            ..Default::default()
                        })
                    }
                    "alias" => {
                        let alias = self.attribute_string(token)?;
                        if alias.is_empty() || alias.contains('\0') {
                            return Err(
                                token.error("alias requires a nonempty target name without NUL")
                            );
                        }
                        Attribute::Symbol(SymbolAttributes {
                            alias: Some(alias),
                            ..Default::default()
                        })
                    }
                    "always_inline" => Attribute::FunctionHint("always_inline"),
                    "noinline" => Attribute::FunctionHint("noinline"),
                    "const" => Attribute::FunctionHint("const"),
                    "pure" => Attribute::FunctionHint("pure"),
                    "cold" => Attribute::FunctionHint("cold"),
                    "warn_unused_result" => Attribute::FunctionHint("warn_unused_result"),
                    "malloc" => {
                        if self.eat("(").is_some() {
                            self.identifier()
                                .ok_or_else(|| self.error("expected a malloc deallocator"))?;
                            if self.eat(",").is_some() {
                                self.attribute_index(depth)?;
                            }
                            self.expect(")", "expected ')' after malloc deallocator")?;
                        }
                        Attribute::FunctionHint("malloc")
                    }
                    "format" => {
                        self.expect("(", "expected '(' after format")?;
                        self.identifier()
                            .ok_or_else(|| self.error("expected a format archetype"))?;
                        self.expect(",", "expected ',' before the format parameter index")?;
                        if self.attribute_index(depth)? == 0 {
                            return Err(token.error("format parameter index must be positive"));
                        }
                        self.expect(",", "expected ',' before the first checked argument index")?;
                        self.attribute_index(depth)?;
                        self.expect(")", "expected ')' after format arguments")?;
                        Attribute::FunctionHint("format")
                    }
                    "deprecated" => {
                        if self.peek().is_some_and(|next| next.is("(")) {
                            self.attribute_string(token)?;
                        }
                        Attribute::Deprecated
                    }
                    // The separately pinned §7.8 already supports this on
                    // AArch64 float and double typedefs.
                    "vector_size" if self.target == Target::Aarch64 => {
                        self.expect("(", "expected '(' after vector_size")?;
                        let expression = self.conditional(depth + 1)?;
                        let bytes = match self.layout_constant(expression)? {
                            8 => 8,
                            16 => 16,
                            _ => {
                                return Err(
                                    token.error("AArch64 short vectors must occupy 8 or 16 bytes")
                                )
                            }
                        };
                        self.expect(")", "expected ')' after the vector size")?;
                        Attribute::VectorSize(bytes)
                    }
                    _ => return Err(token.error(format!("unsupported attribute '{name}'"))),
                };
                attributes.push((token, attribute));
                if self.eat(",").is_none() {
                    self.expect(")", "expected ')' after the attribute")?;
                    break;
                }
            }
            self.expect(")", "expected ')' after the attribute list")?;
        }
        Ok(attributes)
    }

    fn attribute_index(&mut self, depth: usize) -> Result<u64, Diagnostic> {
        let expression = self.conditional(depth + 1)?;
        self.layout_constant(expression)
    }

    fn attribute_string(&mut self, site: &Lexeme) -> Result<String, Diagnostic> {
        self.expect("(", "expected '(' before attribute string argument")?;
        let mut bytes = Vec::new();
        let mut present = false;
        while self.peek().is_some_and(|token| token.kind == Kind::String) {
            present = true;
            crate::sema::decode_string(self.take().unwrap(), &mut bytes)?;
        }
        if !present {
            return Err(site.error("attribute requires a string literal"));
        }
        self.expect(")", "expected ')' after attribute string argument")?;
        String::from_utf8(bytes).map_err(|_| site.error("attribute string must be UTF-8"))
    }
}

impl DeclStorage {
    pub(super) fn apply_attribute(
        &mut self,
        site: &Lexeme,
        attribute: Attribute,
    ) -> Result<(), Diagnostic> {
        match attribute {
            Attribute::Aligned(value) => {
                self.gnu_alignment = self.gnu_alignment.max(value);
            }
            Attribute::Noreturn => self.noreturn = true,
            Attribute::ReturnsTwice => self.returns_twice = true,
            Attribute::Symbol(attributes) => self.symbol.merge(attributes, site)?,
            Attribute::MayAlias => self.may_alias = true,
            Attribute::FunctionHint(name) => self.function_hint = Some(name),
            Attribute::Unused | Attribute::Used | Attribute::Deprecated => {}
            _ => return Err(site.error("attribute does not apply to this declaration")),
        }
        Ok(())
    }
}
