//! Block scopes, control-flow targets and initializers for unoptimized code.

use super::*;
use crate::types::RecordKind;
use std::collections::BTreeMap;

pub(super) struct Label<'a> {
    id: usize,
    definition: bool,
    site: &'a Lexeme,
}

pub(super) struct Switch {
    ty: Type,
    cases: Vec<(Option<u64>, usize)>,
    values: HashSet<Option<u64>>,
}

impl<'a, 's> FunctionBody<'a, 's> {
    pub(super) fn new(
        symbols: &'s HashMap<&'a str, Symbol>,
        result: &'s Type,
        target: Target,
        standard: Standard,
        params: &[Parameter<'a>],
        records: &'s RecordScope<'a, 's>,
        block_externs: &'s mut HashMap<&'a str, Type>,
    ) -> Self {
        let mut scope = HashMap::new();
        let locals = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let (name, site) = param.name.unwrap();
                scope.insert(name, index);
                Local {
                    object: Object {
                        name,
                        site,
                        ty: param.ty.clone(),
                        alignment: None,
                    },
                    storage: LocalStorage::Automatic,
                }
            })
            .collect();
        Self {
            symbols,
            block_externs,
            locals,
            static_literals: None,
            scopes: vec![scope],
            labels: HashMap::new(),
            next_label: 0,
            exit: None,
            repeat: None,
            switches: Vec::new(),
            result,
            last_named_parameter: None,
            target,
            standard,
            records,
        }
    }

    pub(super) fn statements(
        &mut self,
        statements: Vec<parse::Statement<'a>>,
    ) -> Result<Vec<Statement<'a>>, Diagnostic> {
        statements
            .into_iter()
            .map(|statement| self.statement(statement))
            .collect()
    }

    fn next_label(&mut self) -> usize {
        let id = self.next_label;
        self.next_label += 1;
        id
    }

    pub(super) fn validate_labels(&self) -> Result<(), Diagnostic> {
        if let Some(label) = self
            .labels
            .values()
            .filter(|label| !label.definition)
            .min_by_key(|label| label.id)
        {
            return Err(label
                .site
                .error(format!("undefined label '{}'", label.site.text)));
        }
        Ok(())
    }

    fn label(&mut self, token: &'a Lexeme, definition: bool) -> Result<usize, Diagnostic> {
        if let Some(label) = self.labels.get_mut(token.text.as_ref()) {
            if definition && label.definition {
                return Err(token.error("duplicate label"));
            }
            label.definition |= definition;
            return Ok(label.id);
        }
        let id = self.next_label();
        self.labels.insert(
            &token.text,
            Label {
                id,
                definition,
                site: token,
            },
        );
        Ok(id)
    }

    pub(super) fn condition(
        &mut self,
        expr: parse::Expression<'a>,
    ) -> Result<Expression<'a>, Diagnostic> {
        let expr = self.expression(expr)?;
        if !is_scalar(&expr.value_type()) {
            return Err(expr.site.error("condition requires a scalar expression"));
        }
        Ok(expr)
    }

    fn statement(&mut self, statement: parse::Statement<'a>) -> Result<Statement<'a>, Diagnostic> {
        use parse::Statement as Syntax;
        Ok(match statement {
            Syntax::Block(inner) => {
                self.scopes.push(HashMap::new());
                let inner = self.statements(inner)?;
                self.scopes.pop();
                Statement::Block(inner)
            }
            Syntax::Empty => Statement::Empty,
            Syntax::StaticAssert(assertion) => {
                self.static_assertion(assertion)?;
                Statement::Empty
            }
            Syntax::Declaration(vars) => {
                let initializers = vars
                    .into_iter()
                    .map(|var| self.local(var))
                    .collect::<Result<_, _>>()?;
                Statement::Block(initializers)
            }
            Syntax::Expression(expr) => Statement::Expression(self.expression(expr)?),
            Syntax::Return(keyword, expr) => Statement::Return(self.return_value(keyword, expr)?),
            Syntax::If { condition, yes, no } => Statement::If {
                condition: self.condition(condition)?,
                yes: Box::new(self.statement(*yes)?),
                no: no.map(|no| self.statement(*no).map(Box::new)).transpose()?,
            },
            Syntax::Loop {
                condition,
                post_test,
                increment,
                body,
            } => {
                let break_to = self.next_label();
                let continue_to = self.next_label();
                let previous_exit = self.exit.replace(break_to);
                let previous_repeat = self.repeat.replace(continue_to);
                let statement = Statement::Loop {
                    condition: condition.map(|test| self.condition(test)).transpose()?,
                    post_test,
                    increment: increment.map(|expr| self.expression(expr)).transpose()?,
                    body: Box::new(self.statement(*body)?),
                    break_to,
                    continue_to,
                };
                self.exit = previous_exit;
                self.repeat = previous_repeat;
                statement
            }
            Syntax::Switch(value, body) => {
                let value = self.expression(value)?;
                let Kind::Integer(integer) = value.ty.kind else {
                    return Err(value.site.error("switch requires an integer expression"));
                };
                let ty = Type::new(Kind::Integer(value.promoted_integer(integer)));
                let break_to = self.next_label();
                let previous_exit = self.exit.replace(break_to);
                self.switches.push(Switch {
                    ty: ty.clone(),
                    cases: Vec::new(),
                    values: HashSet::new(),
                });
                let body = Box::new(self.statement(*body)?);
                let cases = self.switches.pop().unwrap().cases;
                self.exit = previous_exit;
                Statement::Switch {
                    value,
                    ty,
                    cases,
                    body,
                    break_to,
                }
            }
            Syntax::Case(site, expr, body) => {
                let value = expr
                    .map(|expr| {
                        let expr = self.expression(expr)?;
                        constant_integer(&expr, self.target).ok_or_else(|| {
                            expr.site.error("expected an integer constant case value")
                        })
                    })
                    .transpose()?;
                let id = self.next_label();
                let Some(switch) = self.switches.last_mut() else {
                    return Err(site.error("case label is outside a switch"));
                };
                let value = value.map(|value| canonical(value, &switch.ty, self.target));
                if !switch.values.insert(value) {
                    return Err(site.error("duplicate case value or default"));
                }
                switch.cases.push((value, id));
                Statement::Label(id, Box::new(self.statement(*body)?))
            }
            Syntax::Label(site, statement) => Statement::Label(
                self.label(site, true)?,
                Box::new(self.statement(*statement)?),
            ),
            Syntax::Goto(site) => Statement::Jump(self.label(site, false)?),
            Syntax::Break(site) => Statement::Jump(
                self.exit
                    .ok_or_else(|| site.error("break is outside a loop or switch"))?,
            ),
            Syntax::Continue(site) => Statement::Jump(
                self.repeat
                    .ok_or_else(|| site.error("continue is outside a loop"))?,
            ),
        })
    }

    fn local(&mut self, variable: parse::Variable<'a>) -> Result<Statement<'a>, Diagnostic> {
        if variable.thread_local {
            return self.thread_local(variable);
        }
        let parse::Variable {
            name,
            mut ty,
            storage,
            initializer,
            alignment,
            ..
        } = variable;
        if matches!(ty.kind, Kind::Function { .. }) {
            return self.block_function(name, ty, storage, initializer);
        }
        if storage == Storage::Extern {
            let key = name.text.as_ref();
            if self
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(key))
                .map_or_else(
                    || {
                        self.symbols
                            .get(key)
                            .is_some_and(|symbol| symbol.thread_local)
                    },
                    |&id| matches!(self.locals[id].storage, LocalStorage::ThreadExtern(_)),
                )
                || self.block_externs.contains_key(key)
            {
                return Err(
                    name.error("_Thread_local must appear in every declaration of the object")
                );
            }
            return Err(
                name.error("block-scope extern declarations are outside the current subset")
            );
        }
        let local = self.locals.len();
        if self
            .scopes
            .last_mut()
            .unwrap()
            .insert(&name.text, local)
            .is_some()
        {
            return Err(name.error("duplicate local declaration"));
        }
        self.locals.push(Local {
            object: Object {
                name: &name.text,
                site: &name.site,
                ty: ty.clone(),
                alignment,
            },
            storage: if storage == Storage::Static {
                LocalStorage::Static(None)
            } else {
                LocalStorage::Automatic
            },
        });
        if storage == Storage::Static {
            let encoded = initializer
                .map(|init| self.static_initializer(name, &mut ty, init))
                .transpose()?;
            self.object_size(name, &ty)?;
            self.locals[local].object.ty = ty;
            self.locals[local].storage = LocalStorage::Static(encoded);
            return Ok(Statement::Empty);
        }
        let initializer = initializer
            .map(|init| self.object_initializer(name, &mut ty, init, false))
            .transpose()?;
        self.object_size(name, &ty)?;
        self.locals[local].object.ty = ty;
        Ok(
            initializer.map_or(Statement::Empty, |initializer| Statement::Initialize {
                local,
                initializer,
            }),
        )
    }

    fn block_function(
        &mut self,
        name: &'a Lexeme,
        mut ty: Type,
        storage: Storage,
        initializer: Option<parse::Initializer<'a>>,
    ) -> Result<Statement<'a>, Diagnostic> {
        if !matches!(storage, Storage::Extern | Storage::Unspecified) || initializer.is_some() {
            return Err(name.error("invalid block-scope function declaration"));
        }
        let key = name.text.as_ref();
        let visible = self.scopes.iter().rev().find_map(|scope| scope.get(key));
        let prior = match visible.map(|&id| &self.locals[id]) {
            Some(Local {
                storage: LocalStorage::Function(linkage),
                object,
            }) => Some((&object.ty, *linkage, false)),
            None => self
                .symbols
                .get(key)
                .map(|symbol| (&symbol.ty, symbol.linkage, symbol.defined)),
            _ => None,
        };
        let linkage = if let Some((prior, linkage, defined)) = prior {
            if !compatible_function_declaration(&ty, false, prior, defined) {
                return Err(name.error("incompatible function declaration"));
            }
            if has_prototype(prior) {
                ty = prior.clone();
            }
            linkage
        } else {
            Linkage::External
        };
        let scope = self.scopes.last_mut().unwrap();
        if scope
            .get(key)
            .is_some_and(|&id| !matches!(self.locals[id].storage, LocalStorage::Function(_)))
        {
            return Err(name.error("duplicate local declaration"));
        }
        scope.insert(key, self.locals.len());
        self.locals.push(Local {
            object: Object {
                name: key,
                site: &name.site,
                ty,
                alignment: None,
            },
            storage: LocalStorage::Function(linkage),
        });
        Ok(Statement::Empty)
    }

    pub(super) fn static_initializer(
        &mut self,
        site: &'a Lexeme,
        ty: &mut Type,
        initializer: parse::Initializer<'a>,
    ) -> Result<ThreadInitializer<'a>, Diagnostic> {
        let init = self.object_initializer(site, ty, initializer, true)?;
        thread::encode_initializer(init.values, self.target, &self.locals)
    }

    pub(super) fn compound_literal(
        &mut self,
        site: &'a Lexeme,
        mut ty: Type,
        init: parse::Initializer<'a>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        // N1570 §6.5.2.5p4–6: the same object is used on each evaluation
        // within its lifetime, with its initializer evaluated at the use.
        self.records.resolve(&mut ty, site);
        let kind = if let Some((base, _)) = self.static_literals {
            let initializer = self.static_initializer(site, &mut ty, init)?;
            let (_, literals) = self.static_literals.as_mut().unwrap();
            let id = base + literals.len();
            literals.push(StaticLiteral {
                ty: ty.clone(),
                site: &site.site,
                initializer,
            });
            ExpressionKind::StaticLiteral(id)
        } else {
            let initializer = self.object_initializer(site, &mut ty, init, false)?;
            let local = self.locals.len();
            self.locals.push(Local {
                object: Object {
                    name: "",
                    site: &site.site,
                    ty: ty.clone(),
                    alignment: None,
                },
                storage: LocalStorage::Automatic,
            });
            ExpressionKind::CompoundLiteral { local, initializer }
        };
        Ok((kind, ty))
    }

    fn object_size(&self, site: &Lexeme, ty: &Type) -> Result<usize, Diagnostic> {
        if !supported_object(ty) {
            return Err(site.error("expected a supported object type"));
        }
        let size = ty
            .layout()
            .filter(|layout| layout.size != 0)
            .ok_or_else(|| site.error("expected a complete object type"))?
            .size;
        if size > i32::MAX as usize {
            return Err(site.error("object exceeds the addressable stack or data range"));
        }
        Ok(size)
    }

    fn object_initializer(
        &mut self,
        site: &'a Lexeme,
        ty: &mut Type,
        input: parse::Initializer<'a>,
        static_duration: bool,
    ) -> Result<ObjectInitializer<'a>, Diagnostic> {
        let incomplete_array = matches!(ty.kind, Kind::Array(_, 0));
        if !incomplete_array {
            self.object_size(site, ty)?;
        }
        let clear = matches!(ty.kind, Kind::Array(..))
            || matches!(ty.kind, Kind::Record(_)) && matches!(input, parse::Initializer::List(..));
        let mut plan = InitPlan {
            static_duration,
            ..InitPlan::default()
        };
        let root = InitSlot {
            ty,
            offset: 0,
            bit_field: None,
        };
        self.initialize(input, root, &mut Vec::new(), &mut plan)?;
        if let Kind::Array(_, count @ 0) = &mut ty.kind {
            *count = plan.array_bound;
        }
        let size = self.object_size(site, ty)?;
        Ok(ObjectInitializer {
            values: plan.writes.into_iter().flatten().collect(),
            zero_bytes: if clear { size } else { 0 },
        })
    }

    fn initialize(
        &mut self,
        input: parse::Initializer<'a>,
        slot: InitSlot<'_>,
        path: &mut Vec<usize>,
        plan: &mut InitPlan<'a>,
    ) -> Result<(), Diagnostic> {
        match input {
            parse::Initializer::List(site, mut entries) if !aggregate(slot.ty) => {
                if entries.len() != 1 || !entries[0].designators.is_empty() {
                    return Err(site.error("scalar initializer requires one undesignated value"));
                }
                self.initialize(entries.pop().unwrap().value, slot, path, plan)
            }
            parse::Initializer::List(site, entries) => {
                if entries.len() == 1
                    && entries[0].designators.is_empty()
                    && matches!((&slot.ty.kind, &entries[0].value),
                        (Kind::Array(element, _), parse::Initializer::Expression(expr))
                        if matches!(&expr.kind, parse::ExpressionKind::String(tokens)
                            if string_initializes(element, string_element(tokens, self.target))))
                {
                    return self.initialize(
                        entries.into_iter().next().unwrap().value,
                        slot,
                        path,
                        plan,
                    );
                }
                self.initialize_list(site, entries, slot, path, plan)
            }
            parse::Initializer::Expression(value) => {
                let value = self.expression(value)?;
                self.initialize_value(value, slot, path, plan)
            }
        }
    }

    fn initialize_list(
        &mut self,
        site: &'a Lexeme,
        entries: Vec<parse::InitializerEntry<'a>>,
        root: InitSlot<'_>,
        path: &mut Vec<usize>,
        plan: &mut InitPlan<'a>,
    ) -> Result<(), Diagnostic> {
        // N1570 §6.7.9p17–21: each brace pair starts its own current
        // object. A designation restarts at this root, and a positional
        // entry resumes at the next sibling, climbing out as needed.
        plan.replace(path, root, true);
        let mut current = root.child(0, self.records.records).map(|_| vec![0]);
        for entry in entries {
            let mut selected = if entry.designators.is_empty() {
                current.take().ok_or_else(|| {
                    site.error(if matches!(root.ty.kind, Kind::Array(..)) {
                        "too many array initializers"
                    } else if root.has_flexible_tail(self.records.records) {
                        "cannot initialize a flexible array member"
                    } else {
                        "too many record initializers"
                    })
                })?
            } else {
                self.designated(root, entry.designators)?
            };
            let mut slot = root.at(&selected, self.records.records);
            let start = path.len();
            let entry_value = match entry.value {
                parse::Initializer::Expression(value) => {
                    let value = self.expression(value)?;
                    // Brace elision descends into the first subobject until
                    // the expression can initialize a whole object.
                    while aggregate(slot.ty)
                        && !assignable(slot.ty, &value, self.target)
                        && !string_array_initializer(slot.ty, &value)
                    {
                        if start + selected.len() >= parse::MAX_NESTING {
                            return Err(value.site.error("initializer nesting limit exceeded"));
                        }
                        slot = slot
                            .child(0, self.records.records)
                            .ok_or_else(|| value.site.error("initializer has no subobject"))?;
                        selected.push(0);
                    }
                    Some(value)
                }
                list => {
                    self.select(root, &selected, path, plan);
                    self.initialize(list, slot, path, plan)?;
                    None
                }
            };
            if let Some(value) = entry_value {
                self.select(root, &selected, path, plan);
                self.initialize_value(value, slot, path, plan)?;
            }
            path.truncate(start);
            current = root.next(selected, self.records.records);
        }
        Ok(())
    }

    fn designated(
        &mut self,
        root: InitSlot<'_>,
        designators: Vec<parse::Designator<'a>>,
    ) -> Result<Vec<usize>, Diagnostic> {
        let mut slot = root;
        let mut path = Vec::new();
        for designator in designators {
            let (index, site) = match designator {
                parse::Designator::Index(index) => {
                    if !matches!(slot.ty.kind, Kind::Array(..)) {
                        return Err(index.site.error("array designator requires an array"));
                    }
                    let index = self.expression(index)?;
                    let value = constant_integer(&index, self.target)
                        .filter(|&i| {
                            !matches!(index.ty.kind, Kind::Integer(integer)
                            if integer.is_signed(self.target) && (i as i64) < 0)
                        })
                        .and_then(|i| usize::try_from(i).ok())
                        .ok_or_else(|| {
                            index
                                .site
                                .error("array designator requires a nonnegative integer constant")
                        })?;
                    (value, index.site)
                }
                parse::Designator::Member(name) => {
                    let Kind::Record(record) = slot.ty.kind else {
                        return Err(name.error("member designator requires a struct or union"));
                    };
                    let records = self.records.records;
                    let definition = records[record.id].definition.as_ref().unwrap();
                    let member_path = definition
                        .member_path(&name.text, records)
                        .ok_or_else(|| name.error(format!("no member named '{}'", name.text)))?;
                    for index in member_path {
                        slot = slot.child(index, records).ok_or_else(|| {
                            name.error("cannot initialize a flexible array member")
                        })?;
                        path.push(index);
                    }
                    continue;
                }
            };
            slot = slot
                .child(index, self.records.records)
                .ok_or_else(|| site.error("array designator is outside the object bounds"))?;
            path.push(index);
        }
        Ok(path)
    }

    fn select<'t>(
        &'t self,
        mut root: InitSlot<'t>,
        selected: &[usize],
        path: &mut Vec<usize>,
        plan: &mut InitPlan<'a>,
    ) {
        if path.is_empty() && matches!(root.ty.kind, Kind::Array(_, 0)) {
            plan.array_bound = plan.array_bound.max(selected[0] + 1);
        }
        for &index in selected {
            if root.is_union(self.records.records) {
                let prior = plan.nodes.get(path).and_then(|node| node.union_member);
                if prior != Some(index) {
                    plan.replace(path, root, true);
                    plan.nodes.entry(path.clone()).or_default().union_member = Some(index);
                }
            }
            root = root.child(index, self.records.records).unwrap();
            path.push(index);
        }
    }

    fn initialize_value(
        &self,
        value: Expression<'a>,
        slot: InitSlot<'_>,
        path: &[usize],
        plan: &mut InitPlan<'a>,
    ) -> Result<(), Diagnostic> {
        if string_array_initializer(slot.ty, &value) {
            let (Kind::Array(element, count), ExpressionKind::String(bytes)) =
                (&slot.ty.kind, &value.kind)
            else {
                unreachable!()
            };
            let width = element.layout().unwrap().size;
            let units = bytes.len() / width;
            let count = if *count == 0 {
                plan.array_bound = units;
                units
            } else {
                *count
            };
            if units - 1 > count {
                return Err(value.site.error("string initializer is too long for the array"));
            }
            plan.replace(path, slot, true);
            for (index, unit) in bytes.chunks_exact(width).take(count).enumerate() {
                let mut bits = [0; 8];
                bits[..width].copy_from_slice(unit);
                let mut element_path = path.to_vec();
                element_path.push(index);
                plan.write(
                    &element_path,
                    InitSlot {
                        ty: element,
                        offset: slot.offset + index * width,
                        bit_field: None,
                    },
                    InitValue::Expression(Expression {
                        site: value.site,
                        ty: element.as_ref().clone(),
                        kind: ExpressionKind::Integer(u64::from_le_bytes(bits)),
                    }),
                );
            }
            return Ok(());
        }
        if !assignable(slot.ty, &value, self.target) {
            return Err(value
                .site
                .error(format!("cannot initialize {} with {}", slot.ty, value.ty)));
        }
        if plan.static_duration {
            thread::constant_initializer_value(&value, slot.ty, self.target, &self.locals)?;
        }
        plan.replace(path, slot, false);
        plan.write(path, slot, InitValue::Expression(value));
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct InitSlot<'t> {
    ty: &'t Type,
    offset: usize,
    bit_field: Option<BitField>,
}

impl<'t> InitSlot<'t> {
    fn members(self, records: &'t [Record<'_>]) -> Option<&'t [crate::types::Member<'t>]> {
        let Kind::Record(record) = self.ty.kind else {
            return None;
        };
        Some(&records[record.id].definition.as_ref()?.members)
    }

    fn has_flexible_tail(self, records: &'t [Record<'_>]) -> bool {
        self.members(records)
            .and_then(|members| members.last())
            .is_some_and(|member| matches!(member.ty.kind, Kind::Array(_, 0)))
    }

    fn is_union(self, records: &[Record<'_>]) -> bool {
        matches!(self.ty.kind, Kind::Record(record) if records[record.id].kind == RecordKind::Union)
    }

    fn child(self, index: usize, records: &'t [Record<'_>]) -> Option<Self> {
        if let Kind::Array(element, count) = &self.ty.kind {
            let size = element.layout()?.size;
            let end = index
                .checked_add(1)?
                .checked_mul(size)?
                .checked_add(self.offset)?;
            return ((index < *count || *count == 0) && end <= i32::MAX as usize).then_some(Self {
                ty: element,
                offset: end - size,
                bit_field: None,
            });
        }
        let member = self.members(records)?.get(index)?;
        (!matches!(member.ty.kind, Kind::Array(_, 0))).then_some(Self {
            ty: &member.ty,
            offset: self.offset + member.offset,
            bit_field: member.bit_field,
        })
    }

    fn at(self, path: &[usize], records: &'t [Record<'_>]) -> Self {
        path.iter()
            .fold(self, |slot, &index| slot.child(index, records).unwrap())
    }

    fn next(self, mut path: Vec<usize>, records: &'t [Record<'_>]) -> Option<Vec<usize>> {
        while let Some(index) = path.pop() {
            let parent = self.at(&path, records);
            if !parent.is_union(records) && parent.child(index + 1, records).is_some() {
                path.push(index + 1);
                return Some(path);
            }
        }
        None
    }
}

#[derive(Default)]
struct InitNode {
    write: Option<usize>,
    union_member: Option<usize>,
}

#[derive(Default)]
struct InitPlan<'a> {
    // Keep source order for stores and index them by subobject so replacement
    // touches only the affected subtree, even for large sparse arrays.
    writes: Vec<Option<Initializer<'a>>>,
    nodes: BTreeMap<Vec<usize>, InitNode>,
    array_bound: usize,
    static_duration: bool,
}

impl<'a> InitPlan<'a> {
    fn replace(&mut self, path: &[usize], slot: InitSlot<'_>, clear: bool) {
        let previous: Vec<_> = self
            .nodes
            .range(path.to_vec()..)
            .take_while(|(key, _)| key.starts_with(path))
            .map(|(key, _)| key.clone())
            .collect();
        for key in previous {
            if let Some(index) = self.nodes.remove(&key).unwrap().write {
                self.writes[index] = None;
            }
        }
        // A retained aggregate copy can still supply bytes for this
        // subobject. Restore the implicit initialization required by §6.7.9p19.
        if clear
            && (0..path.len()).any(|len| {
                self.nodes
                    .get(&path[..len])
                    .and_then(|node| node.write)
                    .and_then(|index| self.writes[index].as_ref())
                    .is_some_and(|write| matches!(write.value, InitValue::Expression(_)))
            })
        {
            self.write(path, slot, InitValue::Zero);
        }
    }

    fn write(&mut self, path: &[usize], slot: InitSlot<'_>, value: InitValue<'a>) {
        self.nodes.entry(path.to_vec()).or_default().write = Some(self.writes.len());
        self.writes.push(Some(Initializer {
            offset: slot.offset,
            bit_field: slot.bit_field,
            ty: slot.ty.clone(),
            value,
        }));
    }
}

fn aggregate(ty: &Type) -> bool {
    matches!(ty.kind, Kind::Array(..) | Kind::Record(_))
}

fn string_array_initializer(ty: &Type, value: &Expression<'_>) -> bool {
    matches!(
        (&ty.kind, &value.ty.kind, &value.kind),
        (Kind::Array(element, _), Kind::Array(literal, _), ExpressionKind::String(_))
        if matches!(literal.kind, Kind::Integer(integer) if string_initializes(element, integer))
    )
}

fn supported_object(ty: &Type) -> bool {
    match &ty.kind {
        Kind::Array(element, _) => supported_object(element),
        Kind::Record(_) | Kind::ShortVector { .. } => true,
        _ => is_scalar(ty),
    }
}

// Use the target's integer width when comparing case values or evaluating an
// integer constant expression. These ELF targets both use two's complement.
pub(super) fn canonical(value: u64, ty: &Type, target: Target) -> u64 {
    let Kind::Integer(integer) = ty.kind else {
        return value;
    };
    let shift = 64 - integer.layout().size * 8;
    if integer == Integer::Bool {
        u64::from(value != 0)
    } else if integer.is_signed(target) {
        (((value << shift) as i64) >> shift) as u64
    } else {
        (value << shift) >> shift
    }
}

pub(super) fn constant_integer(expr: &Expression<'_>, target: Target) -> Option<u64> {
    use Binary::*;
    let Kind::Integer(integer) = expr.ty.kind else {
        return None;
    };
    let value = match &expr.kind {
        ExpressionKind::Integer(value) => *value,
        ExpressionKind::Cast(operand) => constant_integer(operand, target).or_else(|| {
            matches!(operand.kind, ExpressionKind::Float(_))
                .then(|| float::constant_to_integer(operand, integer, target))
                .flatten()
        })?,
        ExpressionKind::Unary(op, operand) => {
            let value = constant_integer(operand, target)?;
            match op {
                Unary::Plus => value,
                Unary::Minus => value.wrapping_neg(),
                Unary::Complement => !value,
                Unary::Not => u64::from(value == 0),
                _ => return None,
            }
        }
        ExpressionKind::Binary {
            op,
            left,
            right,
            left_type,
            right_type,
        } => {
            let a = canonical(constant_integer(left, target)?, left_type, target);
            let signed =
                matches!(left_type.kind, Kind::Integer(integer) if integer.is_signed(target));
            if (*op == LogicalAnd && a == 0) || (*op == LogicalOr && a != 0) {
                return Some(u64::from(a != 0));
            }
            let b = canonical(constant_integer(right, target)?, right_type, target);
            let order = if signed {
                (a as i64).cmp(&(b as i64))
            } else {
                a.cmp(&b)
            };
            match op {
                Add => a.wrapping_add(b),
                Subtract => a.wrapping_sub(b),
                Multiply => a.wrapping_mul(b),
                Divide if signed => (a as i64).checked_div(b as i64)? as u64,
                Divide => a.checked_div(b)?,
                Remainder if signed => (a as i64).checked_rem(b as i64)? as u64,
                Remainder => a.checked_rem(b)?,
                ShiftLeft => a.checked_shl(u32::try_from(b).ok()?)?,
                ShiftRight if signed => (a as i64).checked_shr(u32::try_from(b).ok()?)? as u64,
                ShiftRight => a.checked_shr(u32::try_from(b).ok()?)?,
                Less => u64::from(order.is_lt()),
                LessEqual => u64::from(order.is_le()),
                Greater => u64::from(order.is_gt()),
                GreaterEqual => u64::from(order.is_ge()),
                Equal => u64::from(a == b),
                NotEqual => u64::from(a != b),
                BitAnd => a & b,
                BitXor => a ^ b,
                BitOr => a | b,
                LogicalAnd | LogicalOr => u64::from(b != 0),
            }
        }
        ExpressionKind::Conditional { condition, yes, no } => constant_integer(
            if constant_integer(condition, target)? != 0 {
                yes
            } else {
                no
            },
            target,
        )?,
        _ => return None,
    };
    Some(canonical(value, &expr.ty, target))
}
