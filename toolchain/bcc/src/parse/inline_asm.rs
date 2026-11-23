//! GNU C §6.11: literal templates, operands and clobbers.

use super::*;

#[derive(Clone, Debug)]
pub(crate) struct InlineAsm<'a> {
    pub site: &'a Lexeme,
    pub template: String,
    // Basic asm copies the text verbatim; extended asm interprets %.
    pub extended: bool,
    pub outputs: Vec<AsmOperand<'a>>,
    pub inputs: Vec<AsmOperand<'a>>,
}

#[derive(Clone, Debug)]
pub(crate) struct AsmOperand<'a> {
    pub site: &'a Lexeme,
    pub constraint: String,
    pub value: Expression<'a>,
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    pub(super) fn asm_keyword(&self, token: &Lexeme) -> bool {
        matches!(token.text.as_ref(), "__asm" | "__asm__")
            || self.standard.is_gnu() && token.is("asm")
    }

    pub(super) fn inline_asm(&mut self, depth: usize) -> Result<InlineAsm<'a>, Diagnostic> {
        let site = self.take().unwrap();
        if self.peek().is_some_and(|token| {
            matches!(
                token.text.as_ref(),
                "volatile" | "__volatile" | "__volatile__"
            )
        }) {
            self.take();
        }
        if let Some(token) = self.peek().filter(|token| token.kind == Kind::Identifier) {
            return Err(site.error(format!("unsupported asm qualifier '{}'", token.text)));
        }
        self.expect("(", "expected '(' after asm")?;
        let (_, template) = self.asm_string("expected an asm template string")?;
        let extended = self.eat(":").is_some();
        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        if extended {
            outputs = self.asm_operands(depth + 1)?;
            if self.eat(":").is_some() {
                inputs = self.asm_operands(depth + 1)?;
                if self.eat(":").is_some() && !self.peek().is_some_and(|token| token.is(")")) {
                    loop {
                        let (site, clobber) = self.asm_string("expected an asm clobber string")?;
                        if !matches!(clobber.as_str(), "memory" | "cc") {
                            return Err(site.error(format!("unsupported asm clobber '{clobber}'")));
                        }
                        if self.eat(",").is_none() {
                            break;
                        }
                    }
                }
            }
        }
        self.expect(")", "expected ')' after asm operands")?;
        self.expect(";", "expected ';' after asm")?;
        Ok(InlineAsm {
            site,
            template,
            extended,
            outputs,
            inputs,
        })
    }

    fn asm_operands(&mut self, depth: usize) -> Result<Vec<AsmOperand<'a>>, Diagnostic> {
        if self
            .peek()
            .is_some_and(|token| token.is(":") || token.is(")"))
        {
            return Ok(Vec::new());
        }
        let mut operands = Vec::new();
        loop {
            let (site, constraint) =
                self.asm_string("expected an asm operand constraint string")?;
            self.expect("(", "expected '(' before an asm operand")?;
            let value = self.expression(depth)?;
            self.expect(")", "expected ')' after an asm operand")?;
            operands.push(AsmOperand {
                site,
                constraint,
                value,
            });
            if self.eat(",").is_none() {
                return Ok(operands);
            }
        }
    }

    fn asm_string(&mut self, message: &str) -> Result<(&'a Lexeme, String), Diagnostic> {
        let site = self
            .peek()
            .filter(|token| token.kind == Kind::String)
            .ok_or_else(|| self.error(message))?;
        let mut bytes = Vec::new();
        while let Some(token) = self.peek().filter(|token| token.kind == Kind::String) {
            self.take();
            crate::sema::decode_string(token, &mut bytes)?;
        }
        let text = String::from_utf8(bytes).map_err(|_| site.error("asm string is not UTF-8"))?;
        if text.contains('\0') {
            return Err(site.error("NUL in asm string"));
        }
        Ok((site, text))
    }
}
