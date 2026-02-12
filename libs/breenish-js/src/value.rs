//! NaN-boxed JavaScript value representation.
//!
//! Every JS value fits in a single u64 using NaN-boxing:
//! - Regular f64 values are stored as-is
//! - Other types are encoded in the NaN payload space
//!
//! IEEE 754 NaN has the form: 0_11111111111_1xxx...x (sign=0, exponent=all 1s, mantissa!=0)
//! We use the quiet NaN prefix (0x7FF8_0000_0000_0000) as our tag base.
//!
//! Tag layout (bits 48-50 of the NaN payload):
//!   000 = f64 (not a tagged value)
//!   001 = i32 (small integer, payload = value as u32)
//!   010 = bool (payload bit 0 = value)
//!   011 = null
//!   100 = undefined
//!   101 = string (payload = StringId)
//!   110 = object (payload = object index)
//!   111 = (reserved)

use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::string::StringId;

/// Quiet NaN prefix used as tag base.
const QNAN: u64 = 0x7FFC_0000_0000_0000;

/// Tag bits (shifted into bits 48-50).
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NULL: u64 = 0x0003_0000_0000_0000;
const TAG_UNDEFINED: u64 = 0x0004_0000_0000_0000;
const TAG_STRING: u64 = 0x0005_0000_0000_0000;
const TAG_OBJECT: u64 = 0x0006_0000_0000_0000;

/// Mask for extracting the tag bits.
const TAG_MASK: u64 = 0x0007_0000_0000_0000;

/// Mask for extracting the 32-bit payload.
const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF;

/// A JavaScript value packed into 64 bits.
#[derive(Clone, Copy)]
pub struct JsValue(u64);

impl JsValue {
    // --- Constructors ---

    /// Create an undefined value.
    pub const fn undefined() -> Self {
        Self(QNAN | TAG_UNDEFINED)
    }

    /// Create a null value.
    pub const fn null() -> Self {
        Self(QNAN | TAG_NULL)
    }

    /// Create a boolean value.
    pub const fn boolean(v: bool) -> Self {
        Self(QNAN | TAG_BOOL | (v as u64))
    }

    /// Create a number value from f64.
    pub fn number(v: f64) -> Self {
        let bits = v.to_bits();
        // If it's a NaN, canonicalize it so we don't confuse tags
        if v.is_nan() {
            return Self(f64::NAN.to_bits());
        }
        Self(bits)
    }

    /// Create an integer value (small int optimization).
    pub fn int(v: i32) -> Self {
        Self(QNAN | TAG_INT | (v as u32 as u64))
    }

    /// Create a string value from a StringId.
    pub fn string(id: StringId) -> Self {
        Self(QNAN | TAG_STRING | (id.0 as u64))
    }

    /// Create an object value from an object index.
    pub fn object(index: u32) -> Self {
        Self(QNAN | TAG_OBJECT | (index as u64))
    }

    // --- Type checks ---

    fn is_tagged(&self) -> bool {
        (self.0 & QNAN) == QNAN && (self.0 & TAG_MASK) != 0
    }

    fn tag(&self) -> u64 {
        self.0 & TAG_MASK
    }

    fn payload(&self) -> u32 {
        (self.0 & PAYLOAD_MASK) as u32
    }

    pub fn is_number(&self) -> bool {
        !self.is_tagged() || (self.is_tagged() && self.tag() == TAG_INT)
    }

    pub fn is_f64(&self) -> bool {
        !self.is_tagged()
    }

    pub fn is_int(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_INT
    }

    pub fn is_boolean(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_BOOL
    }

    pub fn is_null(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_NULL
    }

    pub fn is_undefined(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_UNDEFINED
    }

    pub fn is_string(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_STRING
    }

    pub fn is_object(&self) -> bool {
        self.is_tagged() && self.tag() == TAG_OBJECT
    }

    pub fn is_nullish(&self) -> bool {
        self.is_null() || self.is_undefined()
    }

    // --- Extraction ---

    /// Get the f64 value. Returns NaN for non-numeric types.
    pub fn as_f64(&self) -> f64 {
        if self.is_int() {
            self.as_i32() as f64
        } else if !self.is_tagged() {
            f64::from_bits(self.0)
        } else {
            f64::NAN
        }
    }

    /// Get the i32 value.
    pub fn as_i32(&self) -> i32 {
        self.payload() as i32
    }

    /// Get the boolean value.
    pub fn as_boolean(&self) -> bool {
        self.payload() != 0
    }

    /// Get the string ID.
    pub fn as_string_id(&self) -> StringId {
        StringId(self.payload())
    }

    /// Get the object index.
    pub fn as_object_index(&self) -> u32 {
        self.payload()
    }

    // --- Type coercion ---

    /// Convert to number (ToNumber abstract operation).
    pub fn to_number(&self) -> f64 {
        if self.is_f64() {
            f64::from_bits(self.0)
        } else if self.is_int() {
            self.as_i32() as f64
        } else if self.is_boolean() {
            if self.as_boolean() { 1.0 } else { 0.0 }
        } else if self.is_null() {
            0.0
        } else if self.is_undefined() {
            f64::NAN
        } else {
            // String and object: NaN for now (proper conversion later)
            f64::NAN
        }
    }

    /// Convert to boolean (ToBoolean abstract operation).
    pub fn to_boolean(&self) -> bool {
        if self.is_boolean() {
            self.as_boolean()
        } else if self.is_f64() {
            let n = f64::from_bits(self.0);
            n != 0.0 && !n.is_nan()
        } else if self.is_int() {
            self.as_i32() != 0
        } else if self.is_null() || self.is_undefined() {
            false
        } else if self.is_string() {
            // Empty string is falsy; we need the string pool to check, so
            // for now assume truthy (non-empty). The VM will handle this properly.
            true
        } else {
            // Objects are always truthy
            true
        }
    }

    /// Get the type name (typeof result).
    pub fn type_name(&self) -> &'static str {
        if self.is_f64() || self.is_int() {
            "number"
        } else if self.is_boolean() {
            "boolean"
        } else if self.is_null() {
            "object" // Yes, typeof null === "object" in JS
        } else if self.is_undefined() {
            "undefined"
        } else if self.is_string() {
            "string"
        } else if self.is_object() {
            "object"
        } else {
            "unknown"
        }
    }

    /// Convert to display string (ToString abstract operation).
    /// For string values, use `to_display_string` with the string pool instead.
    pub fn to_number_string(&self) -> String {
        if self.is_int() {
            format!("{}", self.as_i32())
        } else if self.is_f64() {
            let n = f64::from_bits(self.0);
            if n == n.floor() && n.abs() < 1e15 && !n.is_infinite() {
                // Print integers without decimal point
                format!("{}", n as i64)
            } else {
                format!("{}", n)
            }
        } else if self.is_boolean() {
            if self.as_boolean() {
                String::from("true")
            } else {
                String::from("false")
            }
        } else if self.is_null() {
            String::from("null")
        } else if self.is_undefined() {
            String::from("undefined")
        } else {
            String::from("[value]")
        }
    }

    /// Strict equality (===).
    pub fn strict_equal(&self, other: &JsValue) -> bool {
        if self.is_f64() && other.is_f64() {
            let a = f64::from_bits(self.0);
            let b = f64::from_bits(other.0);
            a == b
        } else if self.is_int() && other.is_int() {
            self.as_i32() == other.as_i32()
        } else if self.is_int() && other.is_f64() {
            (self.as_i32() as f64) == other.as_f64()
        } else if self.is_f64() && other.is_int() {
            self.as_f64() == (other.as_i32() as f64)
        } else {
            self.0 == other.0
        }
    }
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_f64() {
            write!(f, "JsValue({})", f64::from_bits(self.0))
        } else if self.is_int() {
            write!(f, "JsValue(int:{})", self.as_i32())
        } else if self.is_boolean() {
            write!(f, "JsValue({})", self.as_boolean())
        } else if self.is_null() {
            write!(f, "JsValue(null)")
        } else if self.is_undefined() {
            write!(f, "JsValue(undefined)")
        } else if self.is_string() {
            write!(f, "JsValue(str#{})", self.as_string_id().0)
        } else if self.is_object() {
            write!(f, "JsValue(obj#{})", self.as_object_index())
        } else {
            write!(f, "JsValue(0x{:016x})", self.0)
        }
    }
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        self.strict_equal(other)
    }
}
