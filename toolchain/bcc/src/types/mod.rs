//! C type identity and LP64 layout for the hello subset.
use std::fmt;

use crate::pp::{Lexeme, Target};

mod float;
pub(crate) use float::{floor_log10_pow2, FloatFormat};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Qualifiers {
    pub constant: bool,
    pub volatile: bool,
    pub atomic: bool,
    pub restricted: bool,
}

impl Qualifiers {
    pub(crate) fn contains(self, other: Self) -> bool {
        (!other.constant || self.constant) && (!other.volatile || self.volatile)
    }
}

impl fmt::Display for Qualifiers {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.constant {
            out.write_str("const ")?;
        }
        if self.volatile {
            out.write_str("volatile ")?;
        }
        if self.atomic {
            out.write_str("_Atomic ")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Integer {
    Bool,
    Char,
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: usize,
    pub align: usize,
}

pub(crate) const ARM_ATOMIC_RUNTIME: &str = "non-lock-free atomic objects are deferred to ARM 25.9d u3";

impl Layout {
    pub(crate) fn is_lock_free_atomic(self) -> bool {
        matches!(self.size, 1 | 2 | 4 | 8) && self.align >= self.size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    Struct,
    Union,
}

#[derive(Clone, Copy, Debug)]
pub struct RecordType {
    pub id: usize,
    pub layout: Option<Layout>,
}

// Completion changes the available layout, but preserves the tag's identity
// (N1570 §6.7.2.3p4), including inside pointer and function types.
impl PartialEq for RecordType {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RecordType {}

#[derive(Debug)]
pub struct Record<'a> {
    pub kind: RecordKind,
    pub tag: Option<&'a str>,
    pub definition: Option<RecordDefinition<'a>>,
}

// AMD64 psABI §3.5.7, Figure 3.34.
pub(crate) mod amd64_va_list {
    use super::Layout;

    pub(crate) const NAME: &str = "__builtin_va_list";
    pub(crate) const GP_OFFSET: usize = 0;
    pub(crate) const FP_OFFSET: usize = 4;
    pub(crate) const OVERFLOW_ARG_AREA: usize = 8;
    pub(crate) const REG_SAVE_AREA: usize = 16;
    pub(crate) const LAYOUT: Layout = Layout { size: 24, align: 8 };
}

// AAPCS64 2025Q1 §10.1.5 Table 6 and §10.1.1 Table 3 (LP64).
pub(crate) mod aarch64_va_list {
    use super::Layout;

    pub(crate) const NAME: &str = "__va_list";
    pub(crate) const STACK: usize = 0;
    pub(crate) const GR_TOP: usize = 8;
    pub(crate) const VR_TOP: usize = 16;
    pub(crate) const GR_OFFS: usize = 24;
    pub(crate) const VR_OFFS: usize = 28;
    pub(crate) const LAYOUT: Layout = Layout { size: 32, align: 8 };
}

#[derive(Debug)]
pub struct RecordDefinition<'a> {
    pub layout: Layout,
    pub members: Vec<Member<'a>>,
    pub(crate) end: Option<&'a Lexeme>,
    pub(crate) has_const_member: bool,
}

impl RecordDefinition<'_> {
    // N1570 6.7.2.1p13. Keep the path so initializers can still see
    // subobject boundaries and replace the correct union member.
    pub(crate) fn member_path(&self, name: &str, records: &[Record<'_>]) -> Option<Vec<usize>> {
        for (index, member) in self.members.iter().enumerate() {
            if member.name == Some(name) {
                return Some(vec![index]);
            }
            if let (None, Kind::Record(record)) = (member.name, &member.ty.kind) {
                let nested = records[record.id].definition.as_ref().unwrap();
                if let Some(mut path) = nested.member_path(name, records) {
                    path.insert(0, index);
                    return Some(path);
                }
            }
        }
        None
    }
}

#[derive(Debug)]
pub struct Member<'a> {
    // A nameless entry is an anonymous struct or union. Unnamed bit-field
    // padding is consumed by layout and does not appear in this list.
    pub name: Option<&'a str>,
    pub ty: Type,
    pub offset: usize,
    pub bit_field: Option<BitField>,
}

/// The first bit within the member's starting byte and the number of value
/// bits, in little-endian order. Unnamed padding is consumed during layout.
#[derive(Clone, Copy, Debug)]
pub struct BitField {
    pub shift: u8,
    pub width: u8,
    pub(crate) container: Option<BitFieldContainer>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BitFieldContainer {
    pub(crate) back: u8,
    pub(crate) bytes: u8,
}

impl BitField {
    // Describe the bits of the field carried by each byte, including a ninth
    // byte for a packed 64-bit field that does not start on a byte boundary.
    pub(crate) fn bytes(self) -> impl Iterator<Item = (u8, u8, u8)> {
        let mut shift = self.shift;
        let mut used = 0;
        std::iter::from_fn(move || {
            if used == self.width {
                return None;
            }
            let width = (8 - shift).min(self.width - used);
            let mask = ((1u16 << width) - 1) as u8;
            let part = (shift, used, mask);
            used += width;
            shift = 0;
            Some(part)
        })
    }
}

impl Integer {
    pub fn is_signed(self, target: Target) -> bool {
        match self {
            Self::Char => target.signed_char(),
            Self::SignedChar | Self::Short | Self::Int | Self::Long | Self::LongLong => true,
            _ => false,
        }
    }

    // AMD64 psABI §3.1.2, Figure 3.1, and AAPCS64 §5.1, Table 1 and
    // §10.1.1, Table 3. Both targets have the same LP64 integer layout.
    pub fn layout(self) -> Layout {
        let size = match self {
            Self::Bool | Self::Char | Self::SignedChar | Self::UnsignedChar => 1,
            Self::Short | Self::UnsignedShort => 2,
            Self::Int | Self::UnsignedInt => 4,
            Self::Long | Self::UnsignedLong | Self::LongLong | Self::UnsignedLongLong => 8,
        };
        Layout { size, align: size }
    }

    pub(crate) fn promoted(self) -> Self {
        match self {
            Self::Bool
            | Self::Char
            | Self::SignedChar
            | Self::UnsignedChar
            | Self::Short
            | Self::UnsignedShort => Self::Int,
            _ => self,
        }
    }

    pub(crate) fn represents(self, value: u64, target: Target) -> bool {
        let bits = self.layout().size * 8 - usize::from(self.is_signed(target));
        value.checked_shr(bits as u32).is_none_or(|high| high == 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Float {
    Single,
    Double,
}

impl Float {
    pub fn layout(self) -> Layout {
        let size = match self {
            Self::Single => 4,
            Self::Double => 8,
        };
        Layout { size, align: size }
    }
}

impl fmt::Display for Float {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Single => "float",
            Self::Double => "double",
        })
    }
}

impl fmt::Display for Integer {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Bool => "_Bool",
            Self::Char => "char",
            Self::SignedChar => "signed char",
            Self::UnsignedChar => "unsigned char",
            Self::Short => "short",
            Self::UnsignedShort => "unsigned short",
            Self::Int => "int",
            Self::UnsignedInt => "unsigned int",
            Self::Long => "long",
            Self::UnsignedLong => "unsigned long",
            Self::LongLong => "long long",
            Self::UnsignedLongLong => "unsigned long long",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Type {
    pub kind: Kind,
    pub qualifiers: Qualifiers,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Kind {
    Float(Float),
    ShortVector { element: Float, bytes: usize },
    LongDouble,
    Record(RecordType),
    Void,
    Integer(Integer),
    Pointer(Box<Type>),
    Array(Box<Type>, usize),
    Function {
        result: Box<Type>,
        params: Vec<Type>,
        variadic: bool,
        prototype: bool,
    },
}

impl Type {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            qualifiers: Qualifiers::default(),
        }
    }

    pub(crate) fn unqualified(&self) -> Self {
        Self::new(self.kind.clone())
    }

    // Atomic and ordinary objects have distinct types (N1570 6.2.6.1p9).
    // Strip the other qualifiers when checking pointed-to compatibility.
    pub(crate) fn unqualified_except_atomic(&self) -> Self {
        let mut ty = self.unqualified();
        ty.qualifiers.atomic = self.qualifiers.atomic;
        ty
    }

    pub(crate) fn check_atomic(&self, site: &Lexeme) -> Result<(), crate::pp::Diagnostic> {
        if !self.qualifiers.atomic {
            return Ok(());
        }
        match self.kind {
            Kind::Array(..) | Kind::Function { .. } | Kind::Void => {
                Err(site.error("_Atomic requires a non-array object type"))
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn check_atomic_layout(
        &self,
        target: Target,
        site: &crate::pp::Position,
    ) -> Result<(), crate::pp::Diagnostic> {
        if target != Target::Aarch64 {
            return Ok(());
        }
        if self.qualifiers.atomic
            && self.layout().is_some_and(|layout| !layout.is_lock_free_atomic())
        {
            return Err(crate::pp::Diagnostic::new(site, ARM_ATOMIC_RUNTIME));
        }
        if let Kind::Array(element, _) = &self.kind {
            element.check_atomic_layout(target, site)?;
        }
        Ok(())
    }

    // N1570 6.2.7, 6.7.3p10, 6.7.6.2p6, and 6.7.6.3p15.
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        if self.qualifiers != other.qualifiers {
            return false;
        }
        match (&self.kind, &other.kind) {
            (Kind::Pointer(a), Kind::Pointer(b)) => a.compatible(b),
            (Kind::Array(a, m), Kind::Array(b, n)) => {
                (*m == 0 || *n == 0 || m == n) && a.compatible(b)
            }
            (
                Kind::Function {
                    result: a,
                    params: ap,
                    variadic: av,
                    prototype: ax,
                },
                Kind::Function {
                    result: b,
                    params: bp,
                    variadic: bv,
                    prototype: bx,
                },
            ) => {
                if !a.compatible(b) {
                    return false;
                }
                match (ax, bx) {
                    (false, false) => true,
                    (true, true) => {
                        av == bv
                            && ap.len() == bp.len()
                            && ap.iter().zip(bp).all(|(a, b)| a.compatible(b))
                    }
                    (true, false) => prototype_matches_old_style(ap, *av, bp),
                    (false, true) => prototype_matches_old_style(bp, *bv, ap),
                }
            }
            _ => self.kind == other.kind,
        }
    }

    pub(crate) fn default_argument_type(&self) -> Self {
        match self.kind {
            Kind::Integer(integer) => Self::new(Kind::Integer(integer.promoted())),
            Kind::Float(Float::Single) => Self::new(Kind::Float(Float::Double)),
            _ => self.unqualified(),
        }
    }

    pub(crate) fn pointer_to(self) -> Self {
        Self::new(Kind::Pointer(Box::new(self)))
    }

    pub(crate) fn resolve_records(&mut self, layout: &impl Fn(usize) -> Option<Layout>) {
        match &mut self.kind {
            Kind::Record(record) => record.layout = layout(record.id),
            Kind::Pointer(inner) | Kind::Array(inner, _) => inner.resolve_records(layout),
            Kind::Function { result, params, .. } => {
                result.resolve_records(layout);
                for param in params {
                    param.resolve_records(layout);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn qualify_member(&mut self, qualifiers: Qualifiers) {
        if let Kind::Array(element, _) = &mut self.kind {
            element.qualify_member(qualifiers);
        } else {
            self.qualifiers.constant |= qualifiers.constant;
            self.qualifiers.volatile |= qualifiers.volatile;
        }
    }

    pub(crate) fn contains_const(&self, records: &[Record<'_>]) -> bool {
        self.qualifiers.constant
            || match &self.kind {
                Kind::Array(element, _) => element.contains_const(records),
                Kind::Record(record) => records
                    .get(record.id)
                    .and_then(|entry| entry.definition.as_ref())
                    .is_some_and(|definition| definition.has_const_member),
                _ => false,
            }
    }

    /// Natural alignment of the type. Extra placement rules for individual
    /// objects belong to the target backend.
    pub fn layout(&self) -> Option<Layout> {
        match &self.kind {
            Kind::Integer(integer) => Some(integer.layout()),
            Kind::Float(float) => Some(float.layout()),
            Kind::ShortVector { bytes, .. } => Some(Layout {
                size: *bytes,
                align: *bytes,
            }),
            Kind::LongDouble => Some(Layout {
                size: 16,
                align: 16,
            }),
            Kind::Record(record) => record.layout.map(|layout| {
                // Match stage-0 AMD64 Clang's atomic record layout. Larger
                // records keep the representation used by generic libcalls.
                if self.qualifiers.atomic && (1..=16).contains(&layout.size) {
                    let size = layout.size.next_power_of_two();
                    Layout { size, align: size }
                } else {
                    layout
                }
            }),
            // AMD64 psABI Figure 3.1 and AAPCS64 Table 1 (64-bit pointer).
            Kind::Pointer(_) => Some(Layout { size: 8, align: 8 }),
            Kind::Array(element, count) => {
                let element = element.layout()?;
                Some(Layout {
                    size: element.size.checked_mul(*count)?,
                    align: element.align,
                })
            }
            Kind::Void | Kind::Function { .. } => None,
        }
    }

    // N1570 6.7.7: render a type name as a specifier and abstract declarator.
    fn declarator(&self, out: &mut fmt::Formatter<'_>, decl: &str) -> fmt::Result {
        match &self.kind {
            Kind::Void
            | Kind::Integer(_)
            | Kind::Float(_)
            | Kind::ShortVector { .. }
            | Kind::LongDouble
            | Kind::Record(_) => {
                write!(out, "{}", self.qualifiers)?;
                match self.kind {
                    Kind::Integer(integer) => write!(out, "{integer}")?,
                    Kind::Float(float) => write!(out, "{float}")?,
                    Kind::ShortVector { element, bytes } => {
                        write!(out, "{element} __attribute__((vector_size({bytes})))")?
                    }
                    Kind::LongDouble => out.write_str("long double")?,
                    Kind::Record(ref record) => write!(out, "record {}", record.id)?,
                    _ => out.write_str("void")?,
                }
                if !decl.is_empty() {
                    write!(out, " {decl}")?;
                }
                Ok(())
            }
            Kind::Pointer(pointee) => {
                let pointer = format!("*{}{decl}", self.qualifiers);
                let pointer = pointer.trim_end();
                if matches!(pointee.kind, Kind::Array(..) | Kind::Function { .. }) {
                    pointee.declarator(out, &format!("({pointer})"))
                } else {
                    pointee.declarator(out, pointer)
                }
            }
            Kind::Array(element, len) => element.declarator(out, &format!("{decl}[{len}]")),
            Kind::Function {
                result,
                params,
                variadic,
                prototype,
            } => {
                let params = if !prototype {
                    String::new()
                } else if params.is_empty() {
                    "void".into()
                } else {
                    let mut names = params.iter().map(ToString::to_string).collect::<Vec<_>>();
                    if *variadic {
                        names.push("...".into());
                    }
                    names.join(", ")
                };
                result.declarator(out, &format!("{decl}({params})"))
            }
        }
    }
}

fn prototype_matches_old_style(prototype: &[Type], variadic: bool, old: &[Type]) -> bool {
    !variadic
        && if old.is_empty() {
            prototype
                .iter()
                .all(|ty| ty.compatible(&ty.default_argument_type()))
        } else {
            prototype.len() == old.len() && prototype.iter().zip(old).all(|(a, b)| a.compatible(b))
        }
}

impl fmt::Display for Type {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.declarator(out, "")
    }
}
