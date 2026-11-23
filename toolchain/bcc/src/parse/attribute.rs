//! Declaration attributes from the pinned GNU C Common Attributes manual.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Attribute {
    Aligned(usize),
    Packed,
    Noreturn,
    ReturnsTwice,
    VectorSize(usize),
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    pub(super) fn attributes(
        &mut self,
        depth: usize,
    ) -> Result<Vec<(&'a Lexeme, Attribute)>, Diagnostic> {
        let mut attributes = Vec::new();
        while self.eat("__attribute__").is_some() {
            self.expect("(", "expected '(' after __attribute__")?;
            self.expect("(", "expected '(' before the attribute list")?;
            while self.eat(")").is_none() {
                if self.eat(",").is_some() {
                    continue;
                }
                let token = self
                    .take()
                    .ok_or_else(|| self.error("expected an attribute name"))?;
                let name = token.text.strip_prefix("__")
                    .and_then(|name| name.strip_suffix("__"))
                    .unwrap_or(&token.text);
                let attribute = match name {
                    "aligned" => {
                        let alignment = if self.eat("(").is_some() {
                            let expression = self.conditional(depth + 1)?;
                            let value = self.layout_constant(expression)?;
                            self.expect(")", "expected ')' after the alignment")?;
                            if !value.is_power_of_two() || value > 4096 {
                                return Err(token.error("alignment must be a power of two up to 4096"));
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
                    // The separately pinned §7.8 already supports this on
                    // AArch64 float and double typedefs.
                    "vector_size" if self.target == Target::Aarch64 => {
                        self.expect("(", "expected '(' after vector_size")?;
                        let expression = self.conditional(depth + 1)?;
                        let bytes = match self.layout_constant(expression)? {
                            8 => 8,
                            16 => 16,
                            _ => return Err(token.error("AArch64 short vectors must occupy 8 or 16 bytes")),
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
            _ => return Err(site.error("attribute does not apply to this declaration")),
        }
        Ok(())
    }
}
