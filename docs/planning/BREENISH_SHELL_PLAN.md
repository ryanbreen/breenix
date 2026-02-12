# Breenish Shell Plan -- ECMAScript Shell for Breenix

## Status

- **Phase 1**: COMPLETE (PR #191) -- Minimal JavaScript interpreter (`breenish-js`)
  - Lexer, compiler, stack-based VM, NaN-boxed values, string interning
  - 19 passing tests including recursive fibonacci
  - `bsh` binary created, integrated with build system
- **Phase 2**: COMPLETE -- Objects, arrays, functions, closures, control flow, GC
  - Object system: properties, literals, dot/bracket access, nested objects
  - Array system: literals, indexing, length, push/pop/indexOf/join/slice/includes/concat/reverse
  - String methods: indexOf/includes/startsWith/endsWith/trim/toUpperCase/toLowerCase/slice/split/replace/charAt
  - Arrow functions: expression body and block body
  - Switch/case with fallthrough and break
  - for...of loops for arrays
  - Template literal interpolation (${expr})
  - CallMethod opcode for built-in method dispatch
  - Closures: upvalue capture from enclosing scopes, persistent state across calls
  - CreateClosure/LoadUpvalue/StoreUpvalue VM opcodes
  - Mark-sweep GC: traces from roots (stack, globals, call frames), frees unreachable objects
  - 68 passing tests
- **Phase 3**: COMPLETE (PR #193) -- MVP Shell with process execution
  - try/catch/finally with exception handler stack; runtime errors caught by catch blocks
  - Object destructuring: `let { a, b: x } = obj`
  - Array destructuring: `let [a, b] = arr`
  - Spread operator: `f(...args)` via CallSpread opcode
  - Native function infrastructure: Rust functions callable from JavaScript
  - exec(cmd, ...args) -> {exitCode, stdout, stderr, pid} via fork/exec/waitpid
  - cd(), pwd(), which(), readFile(), writeFile(), exit() builtins
  - Auto-exec mode for bare commands; directory-aware prompt
  - 86 passing tests, bsh.elf cross-compiles to 303KB
- **Phase 4**: COMPLETE (PRs #194-195) -- Async/await (Promises, event loop)
  - Promise object: PromiseState (Fulfilled/Rejected/Pending), ObjectKind::Promise
  - Promise.resolve(), Promise.reject(), Promise.all(), Promise.race(), Promise.allSettled()
  - Await opcode: extracts fulfilled value, throws on rejected, passes through non-promises
  - .then()/.catch()/.finally() built-in methods on Promise objects
  - Persistent globals with cross-pool property re-keying for Promise global
  - Async function declarations and async arrow functions
  - WrapPromise opcode for implicit Promise wrapping
  - pipe() native function for pipeline execution
  - 102 passing tests
- **Phase 5**: COMPLETE (PRs #196-201) -- Full shell experience
  - JSON.parse/JSON.stringify with recursive descent JSON parser
  - Math object: floor, ceil, round, abs, min, max, pow, sqrt, random, log, trunc, PI, E
  - Number object: isInteger, isFinite, isNaN, parseInt, parseFloat
  - Global parseInt/parseFloat functions
  - Fixed NaN-boxing QNAN constant for correct null/boolean tag encoding
  - for...in loops (iterating object keys via GetKeys opcode)
  - Proper typeof for booleans, null, undefined (Constant::Boolean/Null/Undefined)
  - glob() native function with * and ? pattern matching
  - env() native function: get/set/enumerate environment variables
  - source command: load and evaluate script files in current context
  - .bshrc startup script: auto-loads /etc/bshrc on REPL start
  - Array HOF methods: map, filter, reduce, forEach, find, some, every, flat
  - call_function_sync helper for synchronous callback invocation
  - Single-parameter arrow functions without parens: `x => expr`
  - Interactive line editing: cursor movement, Home/End, Ctrl+A/E/U/K/W/C/D
  - Command history with Up/Down arrow navigation
  - Raw mode terminal handling via libbreenix termios
  - Tab completion for commands (PATH scan) and filenames (directory listing)
  - Nullish coalescing operator (??) with IsNullish opcode
  - Prefix/postfix increment/decrement operators (++/--)
  - Map and Set collections with full method support (get/set/has/delete/size/clear/keys/values/forEach)
  - do...while loops with continue fix (deferred forward-jump patching)
  - 182 passing tests, bsh v0.5.0 with full shell builtins
  - CI: ecosystem-tests job runs all 182 tests in GitHub Actions (PR #201)
  - aarch64: libbreenix-libc provides environ/pow/log for cross-compilation (PR #202)
  - **Default shell**: init.rs and telnetd.rs launch /bin/bsh instead of /bin/init_shell
- **Phase 6**: PLANNED -- Advanced features (class, regex, modules, Proxy, JIT)

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
