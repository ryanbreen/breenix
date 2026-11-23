//! N1570 6.7.6: declarator modifiers are ordered outward from the name.

use super::*;
use crate::types::{Layout, Member, RecordDefinition, RecordKind, RecordType};

enum Modifier<'a> {
    Pointer(Qualifiers),
    Array(usize),
    Function(Parameters<'a>),
}

struct Parameters<'a> {
    items: Vec<Parameter<'a>>,
    variadic: bool,
    tags: HashMap<&'a str, Tag>,
}

struct Declarator<'a> {
    name: Option<&'a Lexeme>,
    modifiers: Vec<Modifier<'a>>,
}

impl<'a> Declarator<'a> {
    fn ty(&self, mut ty: Type, site: &Lexeme) -> Result<Type, Diagnostic> {
        for modifier in self.modifiers.iter().rev() {
            ty = match modifier {
                Modifier::Pointer(qualifiers) => Type {
                    kind: TypeKind::Pointer(Box::new(ty)),
                    qualifiers: *qualifiers,
                },
                Modifier::Array(size) => {
                    if ty.layout().is_none() {
                        return Err(site.error("array element type is incomplete"));
                    }
                    let ty = Type::new(TypeKind::Array(Box::new(ty), *size));
                    if ty.layout().is_none() {
                        return Err(site.error("array size exceeds the target address space"));
                    }
                    ty
                }
                Modifier::Function(params) => {
                    if matches!(ty.kind, TypeKind::Array(..) | TypeKind::Function { .. }) {
                        return Err(site.error("invalid function result type"));
                    }
                    Type::new(TypeKind::Function {
                        result: Box::new(ty),
                        params: params
                            .items
                            .iter()
                            .map(|param| param.ty.unqualified())
                            .collect(),
                        variadic: params.variadic,
                    })
                }
            };
        }
        Ok(ty)
    }
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    pub(super) fn external(&mut self) -> Result<Vec<Declaration<'a>>, Diagnostic> {
        let (base, storage) = self.specifiers(true)?;
        if self.eat(";").is_some() {
            return Ok(Vec::new());
        }
        let first = self.declarator(false, 0)?;
        if let Some(Modifier::Function(..)) = first.modifiers.first() {
            if storage != Storage::Typedef {
                let name = first.name.unwrap();
                let ty = first.ty(base, name)?;
                let TypeKind::Function { result, .. } = ty.kind else {
                    unreachable!()
                };
                let Some(Modifier::Function(params)) = first.modifiers.into_iter().next() else {
                    unreachable!()
                };
                self.name(name, Name::Ordinary);
                let body = if self.eat(";").is_some() {
                    None
                } else {
                    self.expect("{", "expected ';' or a function body")?;
                    Some(self.scope(|parser| {
                        *parser.tags.last_mut().unwrap() = params.tags;
                        for param in &params.items {
                            if let Some(name) = param.name {
                                parser.name(name, Name::Ordinary);
                            }
                        }
                        parser.block_items(0)
                    })?)
                };
                return Ok(vec![Declaration::Function(Function {
                    name,
                    result: *result,
                    params: params.items,
                    variadic: params.variadic,
                    storage,
                    body,
                })]);
            }
        }
        self.variables(base, storage, first)
            .map(|vars| vars.into_iter().map(Declaration::Object).collect())
    }

    pub(super) fn declaration(&mut self) -> Result<Vec<Variable<'a>>, Diagnostic> {
        let (base, storage) = self.specifiers(true)?;
        if self.eat(";").is_some() {
            return Ok(Vec::new());
        }
        let first = self.declarator(false, 0)?;
        self.variables(base, storage, first)
    }

    fn variables(
        &mut self,
        base: Type,
        storage: Storage,
        mut declarator: Declarator<'a>,
    ) -> Result<Vec<Variable<'a>>, Diagnostic> {
        let mut variables = Vec::new();
        loop {
            let name = declarator.name.unwrap();
            let ty = declarator.ty(base.clone(), name)?;
            if storage == Storage::Typedef {
                if let Some(previous) = self.names.last().unwrap().get(name.text.as_ref()) {
                    if !matches!(previous, Name::Typedef(previous) if *previous == ty) {
                        return Err(name.error("conflicting typedef declaration"));
                    }
                }
                self.name(name, Name::Typedef(ty));
            } else {
                self.name(name, Name::Ordinary);
                let initializer = if self.eat("=").is_some() {
                    Some(self.initializer(0)?)
                } else {
                    None
                };
                variables.push(Variable {
                    name,
                    ty,
                    storage,
                    initializer,
                });
            }
            if self.eat(",").is_none() {
                self.expect(";", "expected ';' after the declaration")?;
                return Ok(variables);
            }
            declarator = self.declarator(false, 0)?;
        }
    }

    fn initializer(&mut self, depth: usize) -> Result<Initializer<'a>, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(self.error("initializer nesting limit exceeded"));
        }
        let Some(site) = self.eat("{") else {
            return Ok(Initializer::Expression(self.expression(0)?));
        };
        let mut values = vec![self.initializer(depth + 1)?];
        while self.eat(",").is_some() {
            if self.eat("}").is_some() {
                return Ok(Initializer::List(site, values));
            }
            values.push(self.initializer(depth + 1)?);
        }
        self.expect("}", "expected '}' after initializer list")?;
        Ok(Initializer::List(site, values))
    }

    fn declarator(
        &mut self,
        abstract_ok: bool,
        depth: usize,
    ) -> Result<Declarator<'a>, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(self.error("declarator nesting limit exceeded"));
        }
        let mut pointers = Vec::new();
        while let Some(star) = self.eat("*") {
            if depth + pointers.len() >= MAX_NESTING {
                return Err(star.error("pointer declarator nesting limit exceeded"));
            }
            let mut qualifiers = Qualifiers::default();
            while self.qualifier(&mut qualifiers) {}
            pointers.push(Modifier::Pointer(qualifiers));
        }
        let mut declarator = if self.eat("(").is_some() {
            let inner = self.declarator(abstract_ok, depth + 1)?;
            self.expect(")", "expected ')' in the declarator")?;
            inner
        } else {
            let name = self.identifier();
            if name.is_none() && !abstract_ok {
                return Err(self.error("expected a declarator name"));
            }
            Declarator {
                name,
                modifiers: Vec::new(),
            }
        };
        loop {
            if declarator.modifiers.len() + pointers.len() >= MAX_NESTING {
                return Err(self.error("declarator nesting limit exceeded"));
            }
            if self.eat("[").is_some() {
                let size = if self.eat("]").is_some() {
                    0
                } else {
                    let size = self.literal_integer()?.0;
                    self.expect("]", "expected ']' after the array bound")?;
                    usize::try_from(size)
                        .ok()
                        .filter(|&size| size != 0)
                        .ok_or_else(|| self.error("array bound must be positive"))?
                };
                declarator.modifiers.push(Modifier::Array(size));
            } else if self.eat("(").is_some() {
                // N1570 §6.2.1p4: prototype tags are visible through this
                // function declarator. A definition carries them into its body.
                let params = self.scope(|parser| {
                    let (items, variadic) = parser.parameters(depth + 1)?;
                    Ok(Parameters {
                        items,
                        variadic,
                        tags: std::mem::take(parser.tags.last_mut().unwrap()),
                    })
                })?;
                declarator.modifiers.push(Modifier::Function(params));
            } else {
                break;
            }
        }
        declarator.modifiers.extend(pointers.into_iter().rev());
        Ok(declarator)
    }

    fn parameters(&mut self, depth: usize) -> Result<(Vec<Parameter<'a>>, bool), Diagnostic> {
        let mut params = Vec::new();
        loop {
            let start = self
                .peek()
                .ok_or_else(|| self.error("expected a parameter type"))?;
            let (base, _) = self.specifiers(false)?;
            let declarator = self.declarator(true, depth)?;
            let mut ty = declarator.ty(base, start)?;
            let name = declarator.name;
            // N1570 6.7.6.3p7–10: adjust array and function parameter types.
            ty = match ty.kind {
                TypeKind::Array(element, _) => element.pointer_to(),
                TypeKind::Function { .. } => ty.pointer_to(),
                TypeKind::Void
                    if params.is_empty()
                        && name.is_none()
                        && ty.qualifiers == Qualifiers::default() =>
                {
                    self.expect(")", "expected ')' after 'void'")?;
                    return Ok((params, false));
                }
                TypeKind::Void => return Err(start.error("parameter has void type")),
                _ => ty,
            };
            if let Some(name) = name {
                self.name(name, Name::Ordinary);
            }
            params.push(Parameter { start, name, ty });
            if self.eat(",").is_none() {
                self.expect(")", "expected ',' or ')' after the parameter")?;
                return Ok((params, false));
            }
            if self.eat("...").is_some() {
                self.expect(")", "expected ')' after '...'")?;
                return Ok((params, true));
            }
        }
    }

    fn literal_integer(&mut self) -> Result<(i128, &'a Lexeme), Diagnostic> {
        let token = self
            .take()
            .ok_or_else(|| self.error("expected an integer constant"))?;
        let value = match self.lookup(&token.text) {
            Some(Name::Enumerator(value)) => Some(i128::from(*value)),
            _ => {
                crate::pp::integer_constant(&token.text).map(|constant| i128::from(constant.value))
            }
        };
        value
            .map(|value| (value, token))
            .ok_or_else(|| token.error("expected an integer constant"))
    }

    pub(super) fn enumeration(&mut self, keyword: &Lexeme) -> Result<Type, Diagnostic> {
        let tag = self.identifier();
        let ty = Type::new(TypeKind::Integer(Integer::Int));
        if self.eat("{").is_none() {
            return tag
                .and_then(|name| {
                    self.tags
                        .iter()
                        .rev()
                        .find_map(|scope| scope.get(name.text.as_ref()))
                })
                .filter(|found| matches!(found, Tag::Enum))
                .map(|_| ty)
                .ok_or_else(|| keyword.error("unknown enumeration tag"));
        }
        if let Some(tag) = tag {
            let scope = self.tags.last_mut().unwrap();
            if scope.contains_key(tag.text.as_ref()) {
                return Err(tag.error("tag is already declared in this scope"));
            }
            scope.insert(&tag.text, Tag::Enum);
        }
        let mut next = Some(0i32);
        loop {
            let name = self
                .identifier()
                .ok_or_else(|| self.error("expected an enumerator name"))?;
            let value = if self.eat("=").is_some() {
                let negative = self.eat("-").is_some();
                let (value, site) = self.literal_integer()?;
                let value = if negative { -value } else { value };
                i32::try_from(value).map_err(|_| site.error("enumerator must fit in int"))?
            } else {
                next.ok_or_else(|| name.error("enumerator must fit in int"))?
            };
            if self.names.last().unwrap().contains_key(name.text.as_ref()) {
                return Err(name.error("duplicate enumerator name"));
            }
            self.name(name, Name::Enumerator(value));
            next = value.checked_add(1);
            if self.eat(",").is_none() || self.peek().is_some_and(|t| t.is("}")) {
                self.expect("}", "expected '}' after the enumerators")?;
                break;
            }
        }
        Ok(ty)
    }

    pub(super) fn record(&mut self, keyword: &'a Lexeme, depth: usize) -> Result<Type, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(keyword.error("record nesting limit exceeded"));
        }
        let kind = if keyword.is("struct") {
            RecordKind::Struct
        } else {
            RecordKind::Union
        };
        let tag = self.identifier();
        let open = self.eat("{");
        if tag.is_none() && open.is_none() {
            return Err(keyword.error("expected a record tag or definition"));
        }

        // N1570 §6.7.2.3p4–8: a definition or bare forward declaration
        // binds in this scope. A use with a declarator searches outward.
        let current_scope = open.is_some() || self.peek().is_some_and(|token| token.is(";"));
        let existing = tag.and_then(|name| {
            if current_scope {
                self.tags.last().unwrap().get(name.text.as_ref())
            } else {
                self.tags
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(name.text.as_ref()))
            }
        });
        let id = match existing {
            Some(&Tag::Record(id)) if self.records[id].kind == kind => id,
            Some(_) => return Err(tag.unwrap().error("tag was declared with a different kind")),
            None => {
                let id = self.records.len();
                self.records.push(Record {
                    kind,
                    tag: tag.map(|name| name.text.as_ref()),
                    definition: None,
                });
                if let Some(tag) = tag {
                    self.tags
                        .last_mut()
                        .unwrap()
                        .insert(&tag.text, Tag::Record(id));
                }
                id
            }
        };
        if let Some(open) = open {
            if self.records[id].definition.is_some() {
                return Err(keyword.error("record type is already defined"));
            }
            let definition = self.record_definition(kind, open, depth)?;
            // Specifiers inside the member list share its surrounding tag
            // scope. A nested definition of this same tag is a redefinition.
            if self.records[id].definition.is_some() {
                return Err(keyword.error("record type is already defined"));
            }
            self.records[id].definition = Some(definition);
        }
        let layout = self.records[id].definition.as_ref().map(|def| def.layout);
        Ok(Type::new(TypeKind::Record(RecordType { id, layout })))
    }

    fn record_definition(
        &mut self,
        kind: RecordKind,
        open: &'a Lexeme,
        depth: usize,
    ) -> Result<RecordDefinition<'a>, Diagnostic> {
        let mut layout = Layout { size: 0, align: 1 };
        let mut members = Vec::new();
        let mut names = std::collections::HashSet::new();
        let mut has_const_member = false;
        let end = loop {
            let (base, _) = self.specifiers_at_depth(false, depth + 1)?;
            loop {
                if let Some(colon) = self.eat(":") {
                    return Err(colon.error("bit-fields are deferred to 25.7b1"));
                }
                let member = self.declarator(false, depth + 1)?;
                let site = member.name.unwrap();
                if let Some(colon) = self.eat(":") {
                    return Err(colon.error("bit-fields are deferred to 25.7b1"));
                }
                if !names.insert(site.text.as_ref()) {
                    return Err(site.error("duplicate record member"));
                }
                let ty = member.ty(base.clone(), site)?;
                let field = ty
                    .layout()
                    .filter(|field| field.size != 0)
                    .ok_or_else(|| site.error("record member type is incomplete"))?;

                // AMD64 psABI §3.1.2 and AAPCS64 §5.10: each member
                // has its natural alignment and the record has the maximum.
                layout.align = layout.align.max(field.align);
                let offset = if kind == RecordKind::Union {
                    layout.size = layout.size.max(field.size);
                    0
                } else {
                    let aligned = layout.size.checked_next_multiple_of(field.align);
                    let Some((offset, size)) =
                        aligned.and_then(|offset| Some((offset, offset.checked_add(field.size)?)))
                    else {
                        return Err(site.error("record size exceeds the target address space"));
                    };
                    layout.size = size;
                    offset
                };
                has_const_member |= ty.contains_const(&self.records);
                members.push(Member {
                    name: &site.text,
                    ty,
                    offset,
                });
                if self.eat(",").is_none() {
                    break;
                }
            }
            self.expect(";", "expected ';' after the record member")?;
            if let Some(end) = self.eat("}") {
                break end;
            }
        };
        layout.size = layout
            .size
            .checked_next_multiple_of(layout.align)
            .ok_or_else(|| open.error("record size exceeds the target address space"))?;
        Ok(RecordDefinition {
            layout,
            members,
            end,
            has_const_member,
        })
    }
}
