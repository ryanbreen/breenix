//! Explicit builtins used by the Breenix scheduler headers.

use super::*;

// GCC Library Builtins §7.1 and N1570 §§7.24.4.1, 7.24.6.1.
// Use the ordinary library call when the explicit builtin has no lowering.
pub(super) fn memory_builtin(name: &str) -> Option<(ExpressionKind<'_>, Type)> {
    let compare = match name {
        "__builtin_memset" => false,
        "__builtin_memcmp" => true,
        _ => return None,
    };
    let int = Type::new(Kind::Integer(Integer::Int));
    let size = Type::new(Kind::Integer(Integer::UnsignedLong));
    let (name, result, params) = if compare {
        let mut constant_void = Type::new(Kind::Void);
        constant_void.qualifiers.constant = true;
        let pointer = constant_void.pointer_to();
        ("memcmp", int, vec![pointer.clone(), pointer, size])
    } else {
        let pointer = Type::new(Kind::Void).pointer_to();
        ("memset", pointer.clone(), vec![pointer, int, size])
    };
    let ty = Type::new(Kind::Function {
        result: Box::new(result),
        params,
        variadic: false,
        prototype: true,
    });
    Some((ExpressionKind::Identifier(name), ty))
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn popcount_long(
        &mut self,
        site: &'a Lexeme,
        mut arguments: Vec<parse::Expression<'a>>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        if arguments.len() != 1 {
            return Err(site.error("__builtin_popcountl requires one argument"));
        }
        let word = Type::new(Kind::Integer(Integer::UnsignedLong));
        let argument = self.expression(arguments.pop().unwrap())?;
        if !assignable(&word, &argument, self.target) {
            return Err(argument.site.error("__builtin_popcountl requires an arithmetic argument"));
        }

        // GCC Bit Operation Builtins §7.2.2: return the count as int.
        // Use a temporary to evaluate the argument once. At each step,
        // add the counts in adjacent groups of shift bits.
        let local = self.locals.len();
        self.locals.push(Local {
            object: Object {
                name: "",
                site: &site.site,
                ty: word.clone(),
                alignment: None,
            },
            storage: LocalStorage::Automatic,
        });
        let expr = |kind| Expression { site, ty: word.clone(), kind };
        let temp = || expr(ExpressionKind::Local(local));
        let integer = |bits| expr(ExpressionKind::Integer(bits));
        let binary = |op, left, right| {
            expr(ExpressionKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                left_type: word.clone(),
                right_type: word.clone(),
            })
        };
        let assign = |right| {
            expr(ExpressionKind::Assignment {
                left: Box::new(temp()),
                right: Box::new(right),
                operation: None,
            })
        };
        let comma = |left, right| expr(ExpressionKind::Comma(Box::new(left), Box::new(right)));
        let mut count = assign(argument);
        for shift in [1, 2, 4, 8, 16, 32] {
            let mask = u64::MAX / ((1u64 << shift) + 1);
            let low = binary(Binary::BitAnd, temp(), integer(mask));
            let shifted = binary(Binary::ShiftRight, temp(), integer(shift));
            let high = binary(Binary::BitAnd, shifted, integer(mask));
            count = comma(count, assign(binary(Binary::Add, low, high)));
        }
        let count = comma(count, temp());
        Ok((
            ExpressionKind::Cast(Box::new(count)),
            Type::new(Kind::Integer(Integer::Int)),
        ))
    }
}
