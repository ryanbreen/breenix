//! N1570 6.7.6: declarator modifiers are ordered outward from the name.

use super::*;
use crate::types::{BitField, Layout, Member, RecordDefinition, RecordKind, RecordType};

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

struct Field<'a> {
    name: Option<&'a Lexeme>,
    site: &'a Lexeme,
    ty: Type,
    width: Option<u8>,
}

impl<'a> Declarator<'a> {
    fn ty(&self, mut ty: Type, site: &Lexeme, records: &[Record<'_>]) -> Result<Type, Diagnostic> {
        for modifier in self.modifiers.iter().rev() {
            ty = match modifier {
                Modifier::Pointer(qualifiers) => Type {
                    kind: TypeKind::Pointer(Box::new(ty)),
                    qualifiers: *qualifiers,
                },
                Modifier::Array(size) => {
                    if ty.layout().is_none_or(|layout| layout.size == 0) {
                        return Err(site.error("array element type is incomplete"));
                    }
                    if contains_flexible_member(&ty, records) {
                        return Err(
                            site.error("array element type contains a flexible array member")
                        );
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
        let (base, storage, signed) = self.specifiers_at_depth(true, 0)?;
        if let Some(end) = self.eat(";") {
            if storage.thread_local {
                return Err(end.error("_Thread_local requires an object declarator"));
            }
            return Ok(Vec::new());
        }
        let first = self.declarator(false, 0)?;
        if let Some(Modifier::Function(..)) = first.modifiers.first() {
            if storage.thread_local {
                return Err(first
                    .name
                    .unwrap()
                    .error("a function cannot be _Thread_local"));
            }
            if storage.class != Storage::Typedef {
                let name = first.name.unwrap();
                let ty = first.ty(base, name, &self.records)?;
                let TypeKind::Function { result, .. } = &ty.kind else {
                    unreachable!()
                };
                let Some(Modifier::Function(params)) = first.modifiers.into_iter().next() else {
                    unreachable!()
                };
                self.name(name, Name::Ordinary(ty.clone()));
                let body = if self.eat(";").is_some() {
                    None
                } else {
                    self.expect("{", "expected ';' or a function body")?;
                    Some(self.scope(|parser| {
                        *parser.tags.last_mut().unwrap() = params.tags;
                        for param in &params.items {
                            if let Some(name) = param.name {
                                parser.name(name, Name::Ordinary(param.ty.clone()));
                            }
                        }
                        parser.block_items(0)
                    })?)
                };
                return Ok(vec![Declaration::Function(Function {
                    name,
                    result: result.as_ref().clone(),
                    params: params.items,
                    variadic: params.variadic,
                    storage: storage.class,
                    body,
                })]);
            }
        }
        self.variables(base, storage, signed, first)
            .map(|vars| vars.into_iter().map(Declaration::Object).collect())
    }

    pub(super) fn declaration(&mut self) -> Result<Vec<Variable<'a>>, Diagnostic> {
        let (base, storage, signed) = self.specifiers_at_depth(true, 0)?;
        if let Some(end) = self.eat(";") {
            if storage.thread_local {
                return Err(end.error("_Thread_local requires an object declarator"));
            }
            return Ok(Vec::new());
        }
        let first = self.declarator(false, 0)?;
        self.variables(base, storage, signed, first)
    }

    fn variables(
        &mut self,
        base: Type,
        storage: DeclStorage,
        signed: bool,
        mut declarator: Declarator<'a>,
    ) -> Result<Vec<Variable<'a>>, Diagnostic> {
        let mut variables = Vec::new();
        loop {
            let name = declarator.name.unwrap();
            let ty = declarator.ty(base.clone(), name, &self.records)?;
            if storage.thread_local && matches!(ty.kind, TypeKind::Function { .. }) {
                return Err(name.error("a function cannot be _Thread_local"));
            }
            if storage.class == Storage::Typedef {
                if let Some(previous) = self.names.last().unwrap().get(name.text.as_ref()) {
                    if !matches!(previous, Name::Typedef(previous, _) if *previous == ty) {
                        return Err(name.error("conflicting typedef declaration"));
                    }
                }
                self.name(name, Name::Typedef(ty, signed));
            } else {
                self.name(name, Name::Ordinary(ty.clone()));
                let initializer = if self.eat("=").is_some() {
                    Some(self.initializer(0)?)
                } else {
                    None
                };
                variables.push(Variable {
                    name,
                    ty,
                    storage: storage.class,
                    thread_local: storage.thread_local,
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
        if let Some(site) = self.eat("{") {
            self.initializer_list(site, depth)
        } else {
            Ok(Initializer::Expression(self.expression(depth)?))
        }
    }

    // N1570 §6.7.9: a designation belongs to one entry of a braced list.
    pub(super) fn initializer_list(
        &mut self,
        site: &'a Lexeme,
        depth: usize,
    ) -> Result<Initializer<'a>, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(site.error("initializer nesting limit exceeded"));
        }
        let mut entries = Vec::new();
        loop {
            let mut designators = Vec::new();
            loop {
                let designator = if self.eat("[").is_some() {
                    let index = self.conditional(depth + 1)?;
                    self.expect("]", "expected ']' after the array designator")?;
                    Designator::Index(index)
                } else if self.eat(".").is_some() {
                    Designator::Member(
                        self.identifier()
                            .ok_or_else(|| self.error("expected a member name after '.'"))?,
                    )
                } else {
                    break;
                };
                if designators.len() == MAX_NESTING {
                    return Err(site.error("designator nesting limit exceeded"));
                }
                designators.push(designator);
            }
            if !designators.is_empty() {
                self.expect("=", "expected '=' after the designation")?;
            }
            entries.push(InitializerEntry {
                designators,
                value: self.initializer(depth + 1)?,
            });
            if self.eat(",").is_none() {
                self.expect("}", "expected '}' after initializer list")?;
                break;
            }
            if self.eat("}").is_some() {
                break;
            }
        }
        Ok(Initializer::List(site, entries))
    }

    pub(super) fn type_name(&mut self, depth: usize) -> Result<Type, Diagnostic> {
        let start = self
            .peek()
            .ok_or_else(|| self.error("expected a type name"))?;
        let (base, _, _) = self.specifiers_at_depth(false, depth)?;
        let declarator = self.declarator(true, depth)?;
        if let Some(name) = declarator.name {
            return Err(name.error("a type name cannot declare an identifier"));
        }
        declarator.ty(base, start, &self.records)
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
            let mut ty = declarator.ty(base, start, &self.records)?;
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
                self.name(name, Name::Ordinary(ty.clone()));
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
        let packed = self.packed_attribute()?;
        let tag = self.identifier();
        let open = self.eat("{");
        if packed && open.is_none() {
            return Err(keyword.error("packed requires a record definition"));
        }
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
            let definition = self.record_definition(kind, packed, open, depth)?;
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

    fn packed_attribute(&mut self) -> Result<bool, Diagnostic> {
        if self.eat("__attribute__").is_none() {
            return Ok(false);
        }
        self.expect("(", "expected '(' after __attribute__")?;
        self.expect("(", "expected '(' before the record type attribute")?;
        let attribute = self
            .identifier()
            .ok_or_else(|| self.error("expected a record type attribute"))?;
        if !matches!(attribute.text.as_ref(), "packed" | "__packed__") {
            return Err(attribute.error("unsupported record type attribute"));
        }
        self.expect(")", "expected ')' after the packed attribute")?;
        self.expect(")", "expected ')' after the attribute list")?;
        Ok(true)
    }

    fn record_definition(
        &mut self,
        kind: RecordKind,
        packed: bool,
        open: &'a Lexeme,
        depth: usize,
    ) -> Result<RecordDefinition<'a>, Diagnostic> {
        let mut fields = Vec::new();
        let mut names = std::collections::HashSet::new();
        let end = loop {
            let (base, _, signed) = self.specifiers_at_depth(false, depth + 1)?;
            loop {
                let (name, mut ty, colon) = if let Some(colon) = self.eat(":") {
                    (None, base.clone(), Some(colon))
                } else {
                    let declarator = self.declarator(false, depth + 1)?;
                    let name = declarator.name.unwrap();
                    (
                        Some(name),
                        declarator.ty(base.clone(), name, &self.records)?,
                        self.eat(":"),
                    )
                };
                let site = name.or(colon).unwrap();
                let width = if let Some(colon) = colon {
                    let TypeKind::Integer(integer) = &mut ty.kind else {
                        return Err(colon.error("bit-fields require an integer type"));
                    };
                    // AMD64 psABI §3.1.2 Figure 3.2 makes a field without
                    // an explicit signed specifier nonnegative.
                    if !signed {
                        *integer = match *integer {
                            Integer::Char => Integer::UnsignedChar,
                            Integer::Short => Integer::UnsignedShort,
                            Integer::Int => Integer::UnsignedInt,
                            Integer::Long => Integer::UnsignedLong,
                            Integer::LongLong => Integer::UnsignedLongLong,
                            other => other,
                        };
                    }
                    let width = self.bit_field_width(*integer, colon, depth + 1)?;
                    if width == 0 && name.is_some() {
                        return Err(site.error("a zero-width bit-field must be unnamed"));
                    }
                    Some(width)
                } else {
                    None
                };
                if let Some(name) = name {
                    if !names.insert(name.text.as_ref()) {
                        return Err(name.error("duplicate record member"));
                    }
                }
                fields.push(Field {
                    name,
                    site,
                    ty,
                    width,
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
        let packed = packed | self.packed_attribute()?;
        self.layout_record(kind, packed, fields, open, end)
    }

    fn layout_record(
        &self,
        kind: RecordKind,
        packed: bool,
        fields: Vec<Field<'a>>,
        open: &'a Lexeme,
        end: &'a Lexeme,
    ) -> Result<RecordDefinition<'a>, Diagnostic> {
        // A cursor in bits represents both sub-byte fields and byte objects.
        // The wider cursor keeps the full range of byte sizes on LP64.
        let mut size_bits = 0u128;
        let mut align = 1;
        let mut members = Vec::new();
        let mut has_const_member = false;
        let mut fields = fields.into_iter().peekable();
        while let Some(Field {
            name,
            site,
            ty,
            width,
        }) = fields.next()
        {
            // N1570 §6.7.2.1p3,18: retain the aligned offset but allocate
            // no elements for the flexible tail.
            let flexible = matches!(ty.kind, TypeKind::Array(_, 0));
            if flexible
                && (kind != RecordKind::Struct || members.is_empty() || fields.peek().is_some())
            {
                return Err(site.error(
                    "a flexible array member must follow another named member and end a struct",
                ));
            }
            if kind == RecordKind::Struct && contains_flexible_member(&ty, &self.records) {
                return Err(
                    site.error("a struct member type cannot contain a flexible array member")
                );
            }
            let field = ty
                .layout()
                .filter(|field| flexible || field.size != 0)
                .ok_or_else(|| site.error("record member type is incomplete"))?;
            let start = if kind == RecordKind::Struct {
                size_bits
            } else {
                0
            };
            let (start, width, bit_field) = if let Some(width) = width {
                let unit = (field.size * 8) as u128;
                // psABI §3.1.2: ordinary fields must fit in an aligned
                // unit of their declared type. Packed fields can straddle
                // that boundary, but the zero-width separator still aligns.
                let start = if width == 0 || !packed && start % unit + u128::from(width) > unit {
                    start.next_multiple_of(unit)
                } else {
                    start
                };
                let bit_field = BitField {
                    shift: (start % 8) as u8,
                    width,
                };
                (start, u128::from(width), Some(bit_field))
            } else {
                let alignment = if packed { 1 } else { field.align };
                (
                    start.next_multiple_of((alignment * 8) as u128),
                    field.size as u128 * 8,
                    None,
                )
            };
            size_bits = size_bits.max(start + width);
            if let Some(name) = name {
                if !packed {
                    align = align.max(field.align);
                }
                let offset = usize::try_from(start / 8)
                    .map_err(|_| site.error("record size exceeds the target address space"))?;
                has_const_member |= ty.contains_const(&self.records);
                members.push(Member {
                    name: &name.text,
                    ty,
                    offset,
                    bit_field,
                });
            }
        }
        let size = usize::try_from(size_bits.div_ceil(8))
            .ok()
            .and_then(|size| size.checked_next_multiple_of(align))
            .ok_or_else(|| open.error("record size exceeds the target address space"))?;
        Ok(RecordDefinition {
            layout: Layout { size, align },
            members,
            end: Some(end),
            has_const_member,
        })
    }

    fn bit_field_width(
        &mut self,
        integer: Integer,
        colon: &'a Lexeme,
        depth: usize,
    ) -> Result<u8, Diagnostic> {
        let expr = self.conditional(depth)?;
        let names = self.names.iter().flatten().map(|(&name, entry)| {
            let ty = match entry {
                Name::Ordinary(ty) => Some(ty),
                _ => None,
            };
            (name, ty)
        });
        let limit = if integer == Integer::Bool {
            1
        } else {
            integer.layout().size * 8
        };
        let width = crate::sema::layout_constant(expr, self.target, &self.records, names)?;
        u8::try_from(width)
            .ok()
            .filter(|width| usize::from(*width) <= limit)
            .ok_or_else(|| colon.error("bit-field width is outside the range of its type"))
    }
}

fn contains_flexible_member(ty: &Type, records: &[Record<'_>]) -> bool {
    let mut pending = vec![ty];
    let mut visited = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        let TypeKind::Record(record) = ty.kind else {
            continue;
        };
        if !visited.insert(record.id) {
            continue;
        }
        if let Some(def) = &records[record.id].definition {
            if def
                .members
                .last()
                .is_some_and(|last| matches!(last.ty.kind, TypeKind::Array(_, 0)))
            {
                return true;
            }
            if records[record.id].kind == RecordKind::Union {
                pending.extend(def.members.iter().map(|member| &member.ty));
            }
        }
    }
    false
}
