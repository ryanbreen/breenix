//! <stdlib.h> and <inttypes.h> functions owned here: memory allocation, numeric
//! conversion, sorting and searching, pseudo-random numbers, integer arithmetic and
//! program termination. The environment functions belong to env.rs and the multibyte
//! functions to locale.

pub(crate) mod alloc_cache;
pub(crate) mod arith;
pub(crate) mod conv;
pub(crate) mod exit;
pub(crate) mod floatconv;
pub(crate) mod malloc;
pub(crate) mod numscan;
pub(crate) mod rand;
pub(crate) mod sort;

pub(crate) const EDOM: i32 = 33;
pub(crate) const EINVAL: i32 = 22;
pub(crate) const ENOMEM: i32 = 12;
pub(crate) const ERANGE: i32 = 34;

pub(crate) use crate::pthread::{errno, set_errno};
