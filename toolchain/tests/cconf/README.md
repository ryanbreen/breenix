# cconf

`cconf` holds expected-output fixtures for C translation phases 1 to 4 and the
hello-subset semantics, and a runner that checks a compiler against them. The
fixtures were written for E25 step 25.5 (unit e1) from ISO/IEC 9899:2011
committee draft N1570 alone (digest in `toolchain/docs/spec-ledger.md`),
before bcc existed. No C compiler or preprocessor source and no compiler test
suite was read to write them. Each fixture cites the clause, paragraph,
EXAMPLE or footnote it checks.

## Running

```sh
python3 toolchain/tests/cconf/run.py --cc /abs/path/to/cc --mode E
python3 toolchain/tests/cconf/run.py --cc /abs/path/to/cc --mode syntax
python3 toolchain/tests/cconf/run.py --cc /abs/path/to/cc --mode types --types-flag FLAG
```

`--target x86_64` (the default) or `--target aarch64` selects the fixtures for
that target. `--stage a1`, `a2` or `b1` (repeatable) limits the run to the
fixtures one 25.5 unit must pass: a1 is phases 1-3, a2 adds clause 6.10, and
b1 is the hello subset. `--cc-arg ARG` (repeatable) puts an argument before
the mode flag. `--list` prints the selected fixtures.

The runner prints `cconf::<id> PASS` or `cconf::<id> FAIL (reason)` for each
fixture and a count. It exits 0 when no selected fixture fails, 1 when one
fails and 2 for a usage error. A `--cc` that does not name an executable file
exits 2 with `compiler not found or not executable` before any fixture runs.
`selftest.py` checks the corpus and the runner with `fake_cc.py`, a test
double that answers from the expectations, and needs no compiler.

## What the runner asks of a compiler

It runs `CC [--cc-arg...] MODE-FLAGS [fixture args] SOURCE` in this directory,
with `SOURCE` relative to it. The mode flags are `-E` or `-fsyntax-only`, and
types mode adds `--types-flag`. Fixture args are only `-I include`. A
`#include` of either form searches the `-I` directories, which is
implementation-defined (6.10.2p2, p3).

**Accepted fixtures** must exit 0 with no `error` diagnostic. Warnings are
allowed.

**`-E` output** is compared as N1570 6.4 pp-tokens, with white space and line
breaks ignored, since their retention is implementation-defined (5.1.1.2p1
phase 3). Adjacent output tokens must stay separate, because the runner splits
the output again. Lines `# N ...` and `#line ...` are line markers and are
dropped. Each `#pragma` line is one item holding its tokens, and its position
among the other tokens counts. bcc keeps a pragma it does not act on as such a
line. Some fixtures list more than one expectation where N1570 leaves the
result unspecified (6.10.3.4p4), and any one of them passes. String literals
are not joined: `-E` stops before phase 6.

**Diagnostics** are lines `FILE:LINE:COLUMN: error: MESSAGE` (or `fatal
error`) on stderr. A negative fixture passes when the compiler exits nonzero
and its first error is at the fixture's location, with `contains` in the
message when the fixture gives it. `FILE` matches when it equals the expected
name or ends in `/` and that name. bcc reports constraint and syntax
violations as errors, though 5.1.1.3 would accept a warning.

**Locations** are bcc's convention, since N1570 defines only presumed line
numbers (6.10.4p2):

- `LINE` is the physical line, one more than the new-lines before the token,
  spliced ones included (6.10.4p2). After `#line` the file and line are the
  presumed ones (6.10.4p4).
- `COLUMN` is the 1-based byte offset in that physical line before trigraph
  replacement, with a tab counting as one. A token that starts after a splice
  takes the column on its own physical line.
- The anchor is the token where the rule fails:
  - a directive's name for `#error` and unbalanced conditionals;
  - the macro name for a redefinition, and an invocation's name for an
    argument-count error;
  - the offending `#` or `##`, parameter or `__VA_ARGS__`;
  - the operator whose result leaves its range;
  - the first argument without a parameter, or the `)` that ends a short list;
  - the first token that cannot continue the parse for a syntax error;
  - the opening `/*` or `"` of an unterminated comment or literal.

`selftest.py` checks that each anchor's text really is at the fixture's
`file:line:column`.

**Type records** (types mode) are JSON objects, one per stdout line. A
declaration is `{"what": "decl", "line", "col", "name", "type", "linkage",
"definition"}`. An expression is `{"what": "expr", "line", "col", "form",
"type", "value"}`, plus `converted` for the type after 6.3.2.1 conversion and
`args` for a call's arguments after conversion to the parameter types. `form`
is `identifier`, `string-literal`, `call` or `integer-constant`. `value` is
`lvalue`, `rvalue` or `function-designator`. Types are 6.7.7 type names, with
qualifiers before the type they qualify, compared as pp-tokens, so spacing is
free: `int (const char *)`, `char [13]`, `int (*)(const char *)`. Each record
in `hello/hello.types.json` must match exactly one dump record on its kind,
position and name or form. Extra dump records are allowed.

## Manifest

`cases.json` lists each fixture with:

- `id`;
- `stage`;
- `mode` (`E`, `syntax` or `types`);
- `clause`, one citation or a list;
- `topics`;
- `source`;
- exactly one of `expect` (alternative expectation files), `accept` or
  `diagnostic` (`file`, `line`, `column`, `at` and optionally `contains`);
- optionally `args`, `targets`, `note` and `physical` (the source line
  behind a presumed location).

A negative fixture's `basis` says why a diagnostic is due:
- `constraint`, a Constraints paragraph;
- `syntax`, a syntax rule (5.1.1.3);
- `error-directive` (4p4, 6.10.5);
- `profile`, which N1570 leaves undefined and bcc chooses to diagnose. It
  carries `profile_reason`.

## Coverage

- **Phases 1-3 (a1).** Trigraphs, including both 5.2.1.1 EXAMPLEs and a `??/`
  splice. Splices inside tokens, including the last-backslash and
  backslash-space cases. Comments: the 6.4.9 EXAMPLE, nesting and delimiters
  inside literals. Two located diagnostics.
- **Clause 6.10 (a2).** Each of its 13 EXAMPLEs: 6.10p8, 6.10.2 EXAMPLE 1 and
  2, 6.10.3.3, 6.10.3.4, 6.10.3.5 EXAMPLE 1 to 7 and 6.10.9. EXAMPLE 6 has its
  valid sequence and one fixture per invalid redefinition.
  <!-- claim-lint:ok: 13 of 13 6.10 EXAMPLEs, toolchain/tests/cconf/selftest.py REQUIRED_EXAMPLES. -->
  Beyond the EXAMPLEs:
  - `#if` arithmetic at the 64-bit `intmax_t` width, and plain-char sign per
    target;
  - `#line`, `_Pragma`, digraph spelling and pp-numbers;
  - recursion suppression and `__LINE__` after splices and comments;
  - constraint and syntax violations, and `#error` locations through splices,
    trigraphs, headers and `#line`.
- **Hello (b1).** `hello/hello.c` accepted, its type records, and nine located
  constraint or syntax violations.

Not covered here:
- the lines of the 6.4.9 EXAMPLE that N1570 makes undefined;
- `__DATE__` and `__TIME__`, whose values change; 25.8a fixes them;
- the system include path;
- 6.4 EXAMPLE 2 (`x+++++y`), checked only in the runner's own pp-token
  splitter because `-E` output cannot show it.

## Black-box cross-check

Homebrew clang 22.1.8 was run through the runner once as a black box (`-std=c11
-nostdinc --target=<arch>-unknown-linux-gnu`). Expectations come from N1570,
not from clang's output.
- **E mode.** It matched 52 of 64 fixtures on each target. It matches every
  6.10 EXAMPLE, the `#if`, `#line` and `_Pragma` fixtures and both char-sign
  fixtures.
- **Splices.** It differs on `backslash-space-is-not-a-splice`: clang splices a
  backslash followed by white space and new-line, and 5.1.1.2p1 phase 2 does
  not.
- **Warnings.** The other 11 differences are diagnostics clang gives as
  warnings or places elsewhere. With `-pedantic-errors`, 59 of 64 match.
- **Anchors.** Two anchors differ by convention: clang puts a too-few-arguments
  error on the `)` and a misplaced `#` on the token after it.
- **Syntax mode.** It matched 7 of 10. clang places the missing-semicolon and
  stray-character errors after the preceding `)` and the bad pp-number at its
  suffix.
- **Types.** Separate `_Generic` assertions confirm the types in
  `hello/hello.types.json` under clang.

## ARM64

Each target selects 64 E-mode fixtures. 63 are shared, and 1 (`if-char-sign`)
differs because plain char is unsigned under AAPCS64. The ARM64 backend uses these target-specific expectations.
