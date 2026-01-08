//! Command-line argument parsing for Breenix userspace programs
//!
//! This module provides utilities to parse argc/argv from the stack,
//! following the Linux x86_64 ABI convention.
//!
//! At process startup, the stack layout is:
//! ```text
//! High addresses:
//!   argv strings (null-terminated)
//!   ...
//!   NULL (end of argv)
//!   argv[n-1] pointer
//!   ...
//!   argv[0] pointer
//!   argc              <- RSP points here at _start
//! Low addresses:
//! ```
//!
//! # Usage
//!
//! In your `_start` function, call `get_args()` to retrieve argc and argv:
//!
//! ```rust,no_run
//! use libbreenix::argv::{get_args, Args};
//!
//! #[no_mangle]
//! pub extern "C" fn _start() -> ! {
//!     let args = unsafe { get_args() };
//!
//!     if args.argc >= 2 {
//!         let filename = args.argv(1);
//!         // Use filename...
//!     }
//!
//!     libbreenix::process::exit(0);
//! }
//! ```

/// Represents the command-line arguments passed to the program.
#[derive(Debug, Clone, Copy)]
pub struct Args {
    /// Number of arguments (argc)
    pub argc: usize,
    /// Pointer to the argv array (array of pointers to null-terminated strings)
    argv_ptr: *const *const u8,
}

impl Args {
    /// Create a new Args from argc and argv pointer
    ///
    /// # Safety
    /// The argv_ptr must point to a valid argv array with at least `argc` entries
    /// followed by a NULL pointer.
    pub const unsafe fn new(argc: usize, argv_ptr: *const *const u8) -> Self {
        Self { argc, argv_ptr }
    }

    /// Get a pointer to argument at index `n`
    ///
    /// Returns NULL if index is out of bounds.
    pub fn argv_raw(&self, n: usize) -> *const u8 {
        if n >= self.argc {
            return core::ptr::null();
        }
        unsafe { *self.argv_ptr.add(n) }
    }

    /// Get argument at index `n` as a byte slice (without null terminator)
    ///
    /// Returns None if index is out of bounds.
    pub fn argv(&self, n: usize) -> Option<&'static [u8]> {
        let ptr = self.argv_raw(n);
        if ptr.is_null() {
            return None;
        }

        // Find the null terminator
        let mut len = 0;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
                // Safety limit
                if len > 4096 {
                    return None;
                }
            }
            Some(core::slice::from_raw_parts(ptr, len))
        }
    }

    /// Get argument at index `n` as a string slice
    ///
    /// Returns None if index is out of bounds or the argument is not valid UTF-8.
    pub fn argv_str(&self, n: usize) -> Option<&'static str> {
        self.argv(n).and_then(|bytes| core::str::from_utf8(bytes).ok())
    }

    /// Check if there are no arguments (shouldn't happen normally)
    pub fn is_empty(&self) -> bool {
        self.argc == 0
    }

    /// Iterator over all arguments as byte slices
    pub fn iter(&self) -> ArgsIter {
        ArgsIter { args: *self, index: 0 }
    }
}

/// Iterator over command-line arguments
pub struct ArgsIter {
    args: Args,
    index: usize,
}

impl Iterator for ArgsIter {
    type Item = &'static [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.args.argc {
            return None;
        }
        let arg = self.args.argv(self.index);
        self.index += 1;
        arg
    }
}

/// Get command-line arguments from the initial stack.
///
/// This function reads argc and argv from the current stack pointer position.
/// It must be called from `_start` before the stack is modified!
///
/// # Safety
///
/// This function must be called from `_start` with the original RSP value.
/// The RSP must point to a valid argc/argv structure set up by the kernel.
///
/// # Usage
///
/// ```rust,no_run
/// #[no_mangle]
/// pub extern "C" fn _start() -> ! {
///     // MUST be the first thing in _start!
///     let args = unsafe { libbreenix::argv::get_args() };
///     // ...
/// }
/// ```
#[inline(never)]
pub unsafe fn get_args() -> Args {
    let argc: usize;
    let argv_ptr: *const *const u8;

    // Read argc from RSP and argv from RSP+8
    // The kernel sets up: [argc] [argv[0]] [argv[1]] ... [NULL]
    // RSP -> argc
    // RSP+8 -> argv[0]
    // RSP+16 -> argv[1]
    // etc.
    core::arch::asm!(
        "mov {argc}, [rsp]",
        "lea {argv}, [rsp + 8]",
        argc = out(reg) argc,
        argv = out(reg) argv_ptr,
        options(nostack, preserves_flags, pure, readonly)
    );

    Args::new(argc, argv_ptr)
}

/// Get command-line arguments from a specific stack pointer.
///
/// This is useful when you need to pass the original RSP from assembly.
///
/// # Safety
///
/// The `stack_ptr` must point to a valid argc/argv structure.
pub unsafe fn get_args_from_stack(stack_ptr: *const u64) -> Args {
    let argc = *stack_ptr as usize;
    let argv_ptr = stack_ptr.add(1) as *const *const u8;
    Args::new(argc, argv_ptr)
}
