//! C type identity and LP64 layout for the hello subset.
use std::fmt;

use crate::pp::Target;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Qualifiers {
    pub constant: bool,
    pub volatile: bool,
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

    pub(crate) fn represents(self, value: u64, target: Target) -> bool {
        let bits = self.layout().size * 8 - usize::from(self.is_signed(target));
        value.checked_shr(bits as u32).is_none_or(|high| high == 0)
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
    Void,
    Integer(Integer),
    Pointer(Box<Type>),
    Array(Box<Type>, usize),
    Function {
        result: Box<Type>,
        params: Vec<Type>,
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

    pub(crate) fn pointer_to(self) -> Self {
        Self::new(Kind::Pointer(Box::new(self)))
    }

    /// Natural alignment of the type. Extra placement rules for individual
    /// objects belong to the target backend.
    pub fn layout(&self) -> Option<Layout> {
        match &self.kind {
            Kind::Integer(integer) => Some(integer.layout()),
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
            Kind::Void | Kind::Integer(_) => {
                write!(out, "{}", self.qualifiers)?;
                match self.kind {
                    Kind::Integer(integer) => write!(out, "{integer}")?,
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
            Kind::Function { result, params } => {
                let params = if params.is_empty() {
                    "void".into()
                } else {
                    params
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                result.declarator(out, &format!("{decl}({params})"))
            }
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.declarator(out, "")
    }
}
