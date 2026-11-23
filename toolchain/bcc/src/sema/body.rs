//! Block scopes, control-flow targets and initializers for unoptimized code.

use super::*;
use crate::types::RecordKind;
use std::iter::Peekable;
use std::vec::IntoIter;

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
        params: &[Parameter<'a>],
        records: &'s RecordScope<'a, 's>,
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
                    },
                    static_storage: false,
                }
            })
            .collect();
        Self {
            symbols,
            locals,
            scopes: vec![scope],
            labels: HashMap::new(),
            next_label: 0,
            exit: None,
            repeat: None,
            switches: Vec::new(),
            result,
            target,
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
        &self,
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
                let ty = Type::new(Kind::Integer(promote(integer)));
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
                        constant_integer(&expr).ok_or_else(|| {
                            expr.site.error("expected an integer constant case value")
                        })
                    })
                    .transpose()?;
                let id = self.next_label();
                let Some(switch) = self.switches.last_mut() else {
                    return Err(site.error("case label is outside a switch"));
                };
                let value = value.map(|value| canonical(value, &switch.ty));
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
        let parse::Variable {
            name,
            mut ty,
            storage,
            initializer,
        } = variable;
        if storage == Storage::Extern {
            return Err(
                name.error("block-scope extern declarations are outside the current subset")
            );
        }
        if storage == Storage::Static && initializer.is_some() {
            return Err(name.error("initialized static objects are outside the current subset"));
        }
        if let Kind::Array(_, count @ 0) = &mut ty.kind {
            let Some(parse::Initializer::List(_, values)) = &initializer else {
                return Err(name.error("incomplete array requires an initializer list"));
            };
            *count = values.len();
        }
        if !supported_object(&ty) {
            return Err(name.error("expected a supported object type"));
        }
        let size = ty
            .layout()
            .filter(|layout| layout.size != 0)
            .ok_or_else(|| name.error("expected a complete object type"))?
            .size;
        if size > i32::MAX as usize {
            return Err(name.error("object exceeds the addressable stack or data range"));
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
            },
            static_storage: storage == Storage::Static,
        });
        if let Some(initializer) = initializer {
            let mut values = Vec::new();
            let input = self.initializer_input(initializer)?;
            let braced = matches!(input, InitializerInput::List(..));
            self.initialize(input, &ty, 0, &mut values)?;
            let zero_bytes = if matches!(ty.kind, Kind::Array(..))
                || braced && matches!(ty.kind, Kind::Record(_))
            {
                size
            } else {
                0
            };
            Ok(Statement::Initialize {
                local,
                values,
                zero_bytes,
            })
        } else {
            Ok(Statement::Empty)
        }
    }

    fn initializer_input(
        &self,
        initializer: parse::Initializer<'a>,
    ) -> Result<InitializerInput<'a>, Diagnostic> {
        Ok(match initializer {
            parse::Initializer::Expression(expr) => InitializerInput::Expression(self.expression(expr)?),
            parse::Initializer::List(site, list) => InitializerInput::List(
                site,
                list.into_iter()
                    .map(|init| self.initializer_input(init))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    fn initialize(
        &self,
        input: InitializerInput<'a>,
        ty: &Type,
        offset: usize,
        values: &mut Vec<Initializer<'a>>,
    ) -> Result<(), Diagnostic> {
        match input {
            InitializerInput::List(site, list) => {
                let mut list = list.into_iter().peekable();
                if aggregate(ty) {
                    self.initialize_subobjects(&mut list, ty, offset, values, 0)?;
                    if list.next().is_some() {
                        let message = if matches!(ty.kind, Kind::Array(..)) {
                            "too many array initializers"
                        } else {
                            "too many record initializers"
                        };
                        return Err(site.error(message));
                    }
                } else if list.len() == 1 {
                    self.initialize(list.next().unwrap(), ty, offset, values)?;
                } else {
                    return Err(site.error("scalar initializer requires one value"));
                }
            }
            InitializerInput::Expression(value) => {
                if !assignable(ty, &value) {
                    return Err(value
                        .site
                        .error(format!("cannot initialize {ty} with {}", value.ty)));
                }
                values.push(Initializer {
                    offset,
                    value,
                    ty: ty.clone(),
                });
            }
        }
        Ok(())
    }

    fn initialize_subobjects(
        &self,
        list: &mut InitializerList<'a>,
        ty: &Type,
        offset: usize,
        values: &mut Vec<Initializer<'a>>,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        // N1570 §6.7.9p17–21: a union selects its first member, and an
        // unbraced subaggregate takes as much of the enclosing list as it needs.
        match &ty.kind {
            Kind::Array(element, length) => {
                let size = element.layout().unwrap().size;
                for index in 0..*length {
                    if list.peek().is_none() {
                        break;
                    }
                    self.initialize_next(list, element, offset + index * size, values, depth)?;
                }
            }
            Kind::Record(record) => {
                let record = &self.records.records[record.id];
                let members = &record.definition.as_ref().unwrap().members;
                let count = if record.kind == RecordKind::Union {
                    1
                } else {
                    members.len()
                };
                for member in &members[..count] {
                    if list.peek().is_none() {
                        break;
                    }
                    self.initialize_next(list, &member.ty, offset + member.offset, values, depth)?;
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn initialize_next(
        &self,
        list: &mut InitializerList<'a>,
        ty: &Type,
        offset: usize,
        values: &mut Vec<Initializer<'a>>,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        if let Some(InitializerInput::Expression(value)) = list.peek() {
            if aggregate(ty) && !assignable(ty, value) {
                if depth == parse::MAX_NESTING {
                    return Err(value.site.error("initializer nesting limit exceeded"));
                }
                return self.initialize_subobjects(list, ty, offset, values, depth + 1);
            }
        }
        self.initialize(list.next().unwrap(), ty, offset, values)
    }
}

enum InitializerInput<'a> {
    Expression(Expression<'a>),
    List(&'a Lexeme, Vec<Self>),
}

type InitializerList<'a> = Peekable<IntoIter<InitializerInput<'a>>>;

fn aggregate(ty: &Type) -> bool {
    matches!(ty.kind, Kind::Array(..) | Kind::Record(_))
}

fn supported_object(ty: &Type) -> bool {
    match &ty.kind {
        Kind::Array(element, _) => supported_object(element),
        Kind::Record(_) => true,
        _ => is_scalar(ty),
    }
}

// Use the target's integer width when comparing case values or evaluating an
// integer constant expression. These ELF targets both use two's complement.
fn canonical(value: u64, ty: &Type) -> u64 {
    let Kind::Integer(integer) = ty.kind else {
        return value;
    };
    let shift = 64 - integer.layout().size * 8;
    if integer == Integer::Bool {
        u64::from(value != 0)
    } else if integer.is_signed(Target::X86_64) {
        (((value << shift) as i64) >> shift) as u64
    } else {
        (value << shift) >> shift
    }
}

pub(super) fn constant_integer(expr: &Expression<'_>) -> Option<u64> {
    use Binary::*;
    if !matches!(expr.ty.kind, Kind::Integer(_)) {
        return None;
    }
    let value = match &expr.kind {
        ExpressionKind::Integer(value) => *value,
        ExpressionKind::Cast(operand) => constant_integer(operand)?,
        ExpressionKind::Unary(op, operand) => {
            let value = constant_integer(operand)?;
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
            let a = canonical(constant_integer(left)?, left_type);
            let signed = matches!(left_type.kind, Kind::Integer(integer) if integer.is_signed(Target::X86_64));
            if (*op == LogicalAnd && a == 0) || (*op == LogicalOr && a != 0) {
                return Some(u64::from(a != 0));
            }
            let b = canonical(constant_integer(right)?, right_type);
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
        ExpressionKind::Conditional { condition, yes, no } => {
            constant_integer(if constant_integer(condition)? != 0 {
                yes
            } else {
                no
            })?
        }
        _ => return None,
    };
    Some(canonical(value, &expr.ty))
}
