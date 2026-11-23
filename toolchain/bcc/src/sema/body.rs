//! Block scopes, control-flow targets and initializers for unoptimized code.

use super::*;
use crate::types::RecordKind;
use std::collections::BTreeMap;

pub(super) struct Label<'a> {
    id: usize,
    definition: Option<Vec<usize>>,
    uses: Vec<(&'a Lexeme, Vec<usize>, Vec<usize>)>,
    vla_scope: Vec<usize>,
    site: &'a Lexeme,
}

pub(super) struct Switch {
    ty: Type,
    cases: Vec<(Option<CaseRange>, usize)>,
    ranges: BTreeMap<u64, u64>,
    has_default: bool,
    region: Option<usize>,
    vla_depth: usize,
}

impl Switch {
    fn add_case(
        &mut self,
        site: &Lexeme,
        value: Option<CaseRange>,
        id: usize,
        is_range: bool,
        target: Target,
    ) -> Result<(), Diagnostic> {
        let duplicate = if let Some(CaseRange { low, high }) = value {
            // Normalize signed order for the interval map. The dispatch
            // list keeps the original bits and source order.
            let sign = if matches!(self.ty.kind, Kind::Integer(i) if i.is_signed(target)) {
                1 << 63
            } else {
                0
            };
            let (low, high) = (low ^ sign, high ^ sign);
            if high < low {
                return Err(site.error("case range has a lower upper bound"));
            }
            let overlaps = self
                .ranges
                .range(..=high)
                .next_back()
                .is_some_and(|(_, end)| *end >= low);
            self.ranges.insert(low, high);
            overlaps
        } else {
            std::mem::replace(&mut self.has_default, true)
        };
        if duplicate {
            return Err(site.error(if is_range {
                "case range overlaps a previous case"
            } else {
                "duplicate case value or default"
            }));
        }
        self.cases.push((value, id));
        Ok(())
    }
}

impl<'a, 's> FunctionBody<'a, 's> {
    pub(super) fn new(
        symbols: &'s HashMap<&'a str, Symbol>,
        result: &'s Type,
        target: Target,
        language: Language,
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
            has_alloca: false,
            alloca_alias: language.alloca_alias,
            static_initialization: false,
            static_literals: None,
            scopes: vec![scope],
            labels: HashMap::new(),
            next_label: 0,
            exit: None,
            repeat: None,
            switches: Vec::new(),
            statement_regions: Vec::new(),
            has_statement_expressions: false,
            jump_regions: HashMap::new(),
            vla_scopes: Vec::new(),
            vla_targets: HashMap::new(),
            vla_locals: HashMap::new(),
            result,
            last_named_parameter: None,
            target,
            standard: language.standard,
            records,
        }
    }

    pub(super) fn statements(
        &mut self,
        statements: Vec<parse::Statement<'a>>,
    ) -> Result<Vec<Statement<'a>>, Diagnostic> {
        let depth = self.vla_scopes.len();
        let mut body = statements
            .into_iter()
            .map(|statement| self.statement(statement))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(&mark) = self.vla_scopes.get(depth) {
            body.push(Statement::RestoreVlaStack(mark));
            self.vla_scopes.truncate(depth);
        }
        Ok(body)
    }

    fn next_label(&mut self) -> usize {
        let id = self.next_label;
        self.next_label += 1;
        self.record_label_region(id);
        id
    }

    fn record_label_region(&mut self, id: usize) {
        if self.vla_scopes.is_empty() {
            self.vla_targets.remove(&id);
        } else {
            self.vla_targets.insert(id, self.vla_scopes.len());
        }
        if let Some(&region) = self.statement_regions.last() {
            self.jump_regions.insert(id, region);
        } else {
            self.jump_regions.remove(&id);
        }
    }

    pub(super) fn validate_labels(&self) -> Result<(), Diagnostic> {
        if let Some(label) = self
            .labels
            .values()
            .filter(|label| label.definition.is_none())
            .min_by_key(|label| label.id)
        {
            return Err(label
                .site
                .error(format!("undefined label '{}'", label.site.text)));
        }
        for label in self.labels.values() {
            if let Some(definition) = &label.definition {
                for (site, region, vla) in &label.uses {
                    if !region.starts_with(definition) {
                        return Err(site.error("goto cannot enter a statement expression"));
                    }
                    if !vla.starts_with(&label.vla_scope) {
                        return Err(site.error("goto cannot enter the scope of a variable length array"));
                    }
                }
            }
        }
        Ok(())
    }

    fn label(&mut self, token: &'a Lexeme, definition: bool) -> Result<usize, Diagnostic> {
        if let Some(label) = self.labels.get_mut(token.text.as_ref()) {
            if definition && label.definition.is_some() {
                return Err(token.error("duplicate label"));
            }
            if definition {
                label.definition = Some(self.statement_regions.clone());
                label.vla_scope.clone_from(&self.vla_scopes);
            } else {
                label.uses.push((token, self.statement_regions.clone(), self.vla_scopes.clone()));
            }
            let id = label.id;
            if definition {
                self.record_label_region(id);
            }
            return Ok(id);
        }
        let id = self.next_label();
        self.labels.insert(
            &token.text,
            Label {
                id,
                definition: definition.then(|| self.statement_regions.clone()),
                uses: if definition {
                    Vec::new()
                } else {
                    vec![(token, self.statement_regions.clone(), self.vla_scopes.clone())]
                },
                vla_scope: if definition { self.vla_scopes.clone() } else { Vec::new() },
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
            Syntax::InlineAsm(asm) => self.inline_asm(asm)?,
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
                // A statement expression in a loop's control clauses
                // sees the enclosing loop, not the one being formed.
                let condition = condition.map(|test| self.condition(test)).transpose()?;
                let increment = increment.map(|expr| self.expression(expr)).transpose()?;
                let previous_exit = self.exit.replace(break_to);
                let previous_repeat = self.repeat.replace(continue_to);
                let statement = Statement::Loop {
                    condition,
                    post_test,
                    increment,
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
                    ranges: BTreeMap::new(),
                    has_default: false,
                    region: self.statement_regions.last().copied(),
                    vla_depth: self.vla_scopes.len(),
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
            Syntax::Case {
                site,
                low,
                high,
                body,
            } => {
                let low = low.map(|expr| self.case_constant(expr)).transpose()?;
                let high = high.map(|expr| self.case_constant(expr)).transpose()?;
                let is_range = high.is_some();
                let id = self.next_label();
                let Some(switch) = self.switches.last_mut() else {
                    return Err(site.error("case label is outside a switch"));
                };
                if switch.region != self.statement_regions.last().copied() {
                    return Err(site.error("case label cannot enter a statement expression"));
                }
                if switch.vla_depth != self.vla_scopes.len() {
                    return Err(site.error("case label cannot enter the scope of a variable length array"));
                }
                let value = low.map(|low| CaseRange {
                    low: canonical(low, &switch.ty, self.target),
                    high: canonical(high.unwrap_or(low), &switch.ty, self.target),
                });
                switch.add_case(site, value, id, is_range, self.target)?;
                Statement::Label(id, Box::new(self.statement(*body)?))
            }
            Syntax::Label(site, statement) => Statement::Label(
                self.label(site, true)?,
                Box::new(self.statement(*statement)?),
            ),
            Syntax::Goto(site) => Statement::Jump(self.label(site, false)?, self.vla_scopes.clone()),
            Syntax::Break(site) => Statement::Jump(
                self.exit
                    .ok_or_else(|| site.error("break is outside a loop or switch"))?,
                self.vla_scopes.clone(),
            ),
            Syntax::Continue(site) => Statement::Jump(
                self.repeat
                    .ok_or_else(|| site.error("continue is outside a loop"))?,
                self.vla_scopes.clone(),
            ),
        })
    }

    fn case_constant(&mut self, expression: parse::Expression<'a>) -> Result<u64, Diagnostic> {
        let expression = self.unevaluated(expression)?;
        constant_integer(&expression, self.target).ok_or_else(|| {
            expression
                .site
                .error("expected an integer constant case value")
        })
    }

    pub(super) fn statement_expression(
        &mut self,
        site: &'a Lexeme,
        mut statements: Vec<parse::Statement<'a>>,
    ) -> Result<Expression<'a>, Diagnostic> {
        if self.static_literals.is_some() {
            return Err(site.error("statement expression is only allowed inside a function"));
        }
        let trailing = if matches!(statements.last(), Some(parse::Statement::Expression(_))) {
            match statements.pop().unwrap() {
                parse::Statement::Expression(expression) => Some(expression),
                _ => unreachable!(),
            }
        } else {
            None
        };
        let id = self.next_label();
        self.has_statement_expressions = true;
        self.statement_regions.push(id);
        self.scopes.push(HashMap::new());
        let statements = self.statements(statements)?;
        let value = trailing
            .map(|expr| self.expression(expr).map(Box::new))
            .transpose()?;
        self.scopes.pop();
        self.statement_regions.pop();
        let ty = value
            .as_ref()
            .map_or_else(|| Type::new(Kind::Void), |expr| expr.value_type());
        Ok(Expression {
            site,
            ty,
            kind: ExpressionKind::StatementExpression {
                region: id,
                statements,
                value,
            },
        })
    }

    fn saved_value(&mut self, site: &'a Lexeme, ty: Type) -> Expression<'a> {
        let id = self.temporary(site, &ty);
        Expression {
            site,
            ty,
            kind: ExpressionKind::Local(id),
        }
    }

    fn save_once(source: Expression<'a>, temporary: Expression<'a>) -> Expression<'a> {
        Expression {
            site: source.site,
            ty: temporary.ty.clone(),
            kind: ExpressionKind::Assignment {
                left: Box::new(temporary),
                right: Box::new(source),
                operation: None,
            },
        }
    }

    pub(super) fn omitted_conditional(
        &mut self,
        site: &'a Lexeme,
        value: parse::Expression<'a>,
        fallback: parse::Expression<'a>,
    ) -> Result<Expression<'a>, Diagnostic> {
        let value = self.condition(value)?;
        let fallback = self.expression(fallback)?;
        let ty = conditional_type(&value, &fallback, self.target)
            .ok_or_else(|| site.error("incompatible conditional operands"))?;
        let saved = self.saved_value(value.site, value.value_type());
        Ok(Expression {
            site,
            ty,
            kind: ExpressionKind::OmittedConditional {
                value: Box::new(value),
                fallback: Box::new(fallback),
                saved: Box::new(saved),
            },
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
            variable_bound,
            ..
        } = variable;
        if let Some(bound) = variable_bound {
            return self.variable_array(name, ty, alignment, bound);
        }
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
            .complete_layout()
            .ok_or_else(|| site.error("expected a complete object type"))?
            .size;
        if size > i32::MAX as usize {
            return Err(site.error("object exceeds the addressable stack or data range"));
        }
        Ok(size)
    }

    pub(super) fn object_initializer(
        &mut self,
        site: &'a Lexeme,
        ty: &mut Type,
        input: parse::Initializer<'a>,
        static_duration: bool,
    ) -> Result<ObjectInitializer<'a>, Diagnostic> {
        let incomplete_array = matches!(ty.kind, Kind::Array(_, None));
        let empty = matches!(&input, parse::Initializer::List(_, entries) if entries.is_empty());
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
        if let Kind::Array(_, count @ None) = &mut ty.kind {
            if plan.array_bound != 0 || empty {
                *count = Some(plan.array_bound);
            }
        }
        let size = self.object_size(site, ty)?;
        Ok(ObjectInitializer {
            values: self.finish_initializer(plan),
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
                let value = self.initializer_expression(value, plan)?;
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
            let selected = if entry.designators.is_empty() {
                vec![current.take().ok_or_else(|| {
                    site.error(if matches!(root.ty.kind, Kind::Array(..)) {
                        "too many array initializers"
                    } else if root.has_flexible_tail(self.records.records) {
                        "cannot initialize a flexible array member"
                    } else {
                        "too many record initializers"
                    })
                })?]
            } else {
                self.designated(root, entry.designators)?
            };
            let outer_range = plan.in_range;
            plan.in_range |= selected.len() > 1;
            let mut selected = selected.into_iter();
            let last = selected.next_back().unwrap();
            for indices in selected {
                self.initialize_entry(entry.value.clone(), root, indices, path, plan)?;
            }
            let last = self.initialize_entry(entry.value, root, last, path, plan)?;
            plan.in_range = outer_range;
            current = root.next(last, self.records.records);
        }
        Ok(())
    }

    fn initialize_entry(
        &mut self,
        input: parse::Initializer<'a>,
        root: InitSlot<'_>,
        mut selected: Vec<usize>,
        path: &mut Vec<usize>,
        plan: &mut InitPlan<'a>,
    ) -> Result<Vec<usize>, Diagnostic> {
        let mut slot = root.at(&selected, self.records.records);
        let start = path.len();
        match input {
            parse::Initializer::Expression(expression) => {
                let value = self.initializer_expression(expression, plan)?;
                // Descend through brace elision as far as the initializer's type requires.
                while aggregate(slot.ty)
                    && !assignable(slot.ty, &value, self.target, self.standard)
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
                self.select(root, &selected, path, plan);
                self.initialize_value(value, slot, path, plan)?;
            }
            list => {
                self.select(root, &selected, path, plan);
                self.initialize(list, slot, path, plan)?;
            }
        }
        path.truncate(start);
        Ok(selected)
    }

    fn initializer_expression(
        &mut self,
        expression: parse::Expression<'a>,
        plan: &mut InitPlan<'a>,
    ) -> Result<Expression<'a>, Diagnostic> {
        let key = std::ptr::from_ref(expression.site);
        if plan.in_range {
            if let Some(value) = plan.range_values.get(&key) {
                return Ok(value.clone());
            }
        }
        let previous = std::mem::replace(&mut self.static_initialization, plan.static_duration);
        let value = self.expression(expression);
        self.static_initialization = previous;
        let value = value?;
        if plan.in_range {
            plan.range_values.insert(key, value.clone());
        }
        Ok(value)
    }

    fn finish_initializer(&mut self, plan: InitPlan<'a>) -> Vec<Initializer<'a>> {
        let mut saved = HashMap::new();
        let mut uses = HashMap::<_, usize>::new();
        if plan.static_duration || plan.range_writes.is_empty() {
            return plan.writes.into_iter().flatten().collect();
        }
        for (&index, &key) in &plan.range_writes {
            if plan.writes[index].is_some() {
                *uses.entry(key).or_default() += 1;
            }
        }
        let mut writes = plan.writes;
        // Overridden stores are already gone. The first surviving use
        // evaluates an initializer, later uses read the same saved value.
        for (index, write) in writes.iter_mut().enumerate() {
            let Some(Initializer {
                value: InitValue::Expression(value),
                ..
            }) = write
            else {
                continue;
            };
            let Some(&key) = plan.range_writes.get(&index) else {
                continue;
            };
            if uses[&key] < 2 {
                continue;
            }
            match saved.entry(key) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    *value = Expression::clone(entry.get());
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let temporary = self.saved_value(value.site, value.value_type());
                    let source = std::mem::replace(value, temporary.clone());
                    let save = Self::save_once(source, temporary.clone());
                    value.kind = ExpressionKind::Comma(Box::new(save), Box::new(temporary.clone()));
                    entry.insert(temporary);
                }
            }
        }
        writes.into_iter().flatten().collect()
    }

    fn designated(
        &mut self,
        root: InitSlot<'_>,
        designators: Vec<parse::Designator<'a>>,
    ) -> Result<Vec<Vec<usize>>, Diagnostic> {
        let mut paths = vec![(root, Vec::new())];
        for designator in designators {
            use parse::Designator;
            match designator {
                Designator::Index(index) => {
                    let (index, site) = self.designator_index(index)?;
                    paths =
                        Self::designator_range(self.records.records, paths, index, index, site)?;
                }
                Designator::Range(low, high) => {
                    let (low, _) = self.designator_index(low)?;
                    let (high, site) = self.designator_index(high)?;
                    paths = Self::designator_range(self.records.records, paths, low, high, site)?;
                }
                Designator::Member(name) => {
                    for (slot, path) in &mut paths {
                        let Kind::Record(record) = slot.ty.kind else {
                            return Err(name.error("member designator requires a struct or union"));
                        };
                        let records = self.records.records;
                        let definition = records[record.id].definition.as_ref().unwrap();
                        let member_path =
                            definition.member_path(&name.text, records).ok_or_else(|| {
                                name.error(format!("no member named '{}'", name.text))
                            })?;
                        for index in member_path {
                            *slot = slot.child(index, records).ok_or_else(|| {
                                name.error("cannot initialize a flexible array member")
                            })?;
                            path.push(index);
                        }
                    }
                }
            }
        }
        Ok(paths.into_iter().map(|(_, path)| path).collect())
    }

    fn designator_index(
        &mut self,
        expression: parse::Expression<'a>,
    ) -> Result<(usize, &'a Lexeme), Diagnostic> {
        let index = self.unevaluated(expression)?;
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
        Ok((value, index.site))
    }

    fn designator_range<'t>(
        records: &'t [Record<'_>],
        paths: Vec<(InitSlot<'t>, Vec<usize>)>,
        low: usize,
        high: usize,
        site: &Lexeme,
    ) -> Result<Vec<(InitSlot<'t>, Vec<usize>)>, Diagnostic> {
        if high < low {
            return Err(site.error("range designator has a lower upper bound"));
        }
        for (slot, _) in &paths {
            if !matches!(slot.ty.kind, Kind::Array(..)) {
                return Err(site.error("array designator requires an array"));
            }
            if slot.child(high, records).is_none() {
                return Err(site.error("array designator is outside the object bounds"));
            }
        }
        let mut selected = Vec::new();
        // bcc represents an initializer as one write per subobject. Bound
        // the number of writes a single designation can synthesize.
        let count = paths
            .len()
            .checked_mul(high - low + 1)
            .filter(|count| *count <= 1_048_576)
            .ok_or_else(|| {
                site.error("range designator exceeds the initializer expansion limit")
            })?;
        selected
            .try_reserve(count)
            .map_err(|_| site.error("range designator exceeds the initializer expansion limit"))?;
        for (slot, path) in paths {
            for index in low..=high {
                let mut path = path.clone();
                path.push(index);
                selected.push((slot.child(index, records).unwrap(), path));
            }
        }
        Ok(selected)
    }

    fn select<'t>(
        &'t self,
        mut root: InitSlot<'t>,
        selected: &[usize],
        path: &mut Vec<usize>,
        plan: &mut InitPlan<'a>,
    ) {
        if path.is_empty() && matches!(root.ty.kind, Kind::Array(_, None)) {
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
            let count = count.unwrap_or_else(|| {
                plan.array_bound = units;
                units
            });
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
        if !assignable(slot.ty, &value, self.target, self.standard) {
            return Err(value
                .site
                .error(format!("cannot initialize {} with {}", slot.ty, value.ty)));
        }
        if plan.static_duration {
            thread::constant_initializer_value(&value, slot.ty, self.target, &self.locals)?;
        }
        plan.replace(path, slot, false);
        if plan.in_range {
            plan.range_writes
                .insert(plan.writes.len(), std::ptr::from_ref(value.site));
        }
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
            .is_some_and(|member| matches!(member.ty.kind, Kind::Array(_, None)))
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
            return (count.is_none_or(|count| index < count) && end <= i32::MAX as usize)
                .then_some(Self {
                    ty: element,
                    offset: end - size,
                    bit_field: None,
                });
        }
        let member = self.members(records)?.get(index)?;
        (!matches!(member.ty.kind, Kind::Array(_, None))).then_some(Self {
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
    in_range: bool,
    range_values: HashMap<*const Lexeme, Expression<'a>>,
    range_writes: HashMap<usize, *const Lexeme>,
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
pub(crate) fn canonical(value: u64, ty: &Type, target: Target) -> u64 {
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

// This is the 6.6 integer constant expression evaluator. Runtime optimization
// must consult TranslationUnit::preserves_float_environment before replacing
// expressions, including integer results of floating-point conversions.
pub(crate) fn constant_integer(expr: &Expression<'_>, target: Target) -> Option<u64> {
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
        ExpressionKind::OmittedConditional {
            value, fallback, ..
        } => constant_integer(
            if float::constant_truth(value, target)? {
                value
            } else {
                fallback
            },
            target,
        )?,
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
