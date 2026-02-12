# Breenish Shell Plan -- ECMAScript Shell for Breenix

## Status

- **Phase 1**: COMPLETE (PR #191) -- Minimal JavaScript interpreter (`breenish-js`)
  - Lexer, compiler, stack-based VM, NaN-boxed values, string interning
  - 19 passing tests including recursive fibonacci
  - `bsh` binary created, integrated with build system
- **Phase 2**: NEARLY COMPLETE -- Objects, arrays, functions, control flow
  - Object system: properties, literals, dot/bracket access, nested objects
  - Array system: literals, indexing, length, push/pop/indexOf/join/slice/includes/concat/reverse
  - String methods: indexOf/includes/startsWith/endsWith/trim/toUpperCase/toLowerCase/slice/split/replace/charAt
  - Arrow functions: expression body and block body
  - Switch/case with fallthrough and break
  - for...of loops for arrays
  - Template literal interpolation (${expr})
  - CallMethod opcode for built-in method dispatch
  - 59 passing tests
  - Remaining: closures (upvalues), GC (reference counting)
- **Phase 3**: PLANNED -- Process execution (exec, pipe, env)
- **Phase 4**: PLANNED -- Async/await (Promises, event loop)
- **Phase 5**: PLANNED -- Full shell experience (line editing, completion, modules)
- **Phase 6**: PLANNED -- Advanced features (class, Proxy, JIT)

## Architecture

### Crate Structure

```
libs/breenish-js/           # JS engine (no_std + alloc capable, std by default)
  src/
    lib.rs                  # Public API: Context, eval()
    lexer.rs                # Tokenizer
    token.rs                # Token types
    compiler.rs             # Direct source-to-bytecode compiler (no AST)
    bytecode.rs             # Opcode definitions, CodeBlock
    vm.rs                   # Stack-based interpreter loop
    value.rs                # NaN-boxed JsValue
    object.rs               # Object/array heap and property storage
    string.rs               # String interning table
    error.rs                # JS Error types

userspace/programs/src/
  bsh.rs                    # The Breenish shell binary
```

### Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Compilation | Direct-to-bytecode (no AST) | QuickJS model. Shell scripts parse once, run once. |
| VM type | Stack-based | Simpler to implement than register-based. |
| GC strategy | Reference counting + cycle detection (Phase 2+) | Deterministic, low latency. |
| Value repr | NaN-boxing (64-bit tagged) | Industry standard. One u64 per value. |
| std dependency | std feature flag | Engine is no_std-capable, shell binary uses std. |

## Phase Details

See the full plan in the conversation that created this document.

## Verification

- Phase 1: `echo 'let x = 1 + 2; print(x);' | bsh` prints `3`
- Phase 2: `echo 'function fib(n) { if (n <= 1) return n; return fib(n-1)+fib(n-2); } print(fib(10));' | bsh` prints `55`
- Phase 3: Interactive REPL: type `exec("ls", "/bin")` and see file listing
- Phase 4: `echo 'let r = await exec("echo", "hello"); print(r.stdout);' | bsh` prints `hello`
- Phase 5: `bsh` boots as the default shell, loads `.bshrc`, supports tab completion
