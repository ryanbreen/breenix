# C and Rust interoperability

The shared records use `#[repr(C)]` and C-compatible field types. Their fields
are laid out in declaration order, with each field aligned as the target C ABI
requires; the record alignment is its most aligned member's alignment, and its
size includes tail padding to that alignment. For `Mixed { int, double }` this
means offsets 0 and 8, size 16 and alignment 8 on both targets. These layout
requirements follow AMD64 psABI §3.1.2 and AAPCS64 §5.3.5 and §5.10.1.

Both sides declare functions and callbacks with the C calling convention:
Rust uses `extern "C"`, and C uses the corresponding function prototype. A
record passed or returned by value follows the target's aggregate classification
and register allocation, not Rust's native calling convention. AMD64 psABI
§3.2.3 classifies the integer and double eightbytes separately; AAPCS64 §6.4
and §6.8 specify the parameter and result conventions. A callback is a function
pointer using that same convention in both directions.

On x86-64, floating-point fields cross this boundary only when the Rust side is
built for `x86_64-breenix-c.json`, whose SSE and SysV float ABI match bcc.
The ordinary `x86_64-breenix.json` target uses the soft-float Rust ABI and is
not interchangeable for this interface. AArch64 uses its C-compatible target;
its bcc emission remains scheduled for A3 and A4.
