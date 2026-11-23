# bcc C11 coverage for E25 25.9

Clause and paragraph numbers refer to the pinned WG14 N1570 draft in
[spec-ledger.md](spec-ledger.md). This inventory records what the corpus
checks and where coverage is open. A compile-fail fixture demonstrates
one diagnosed violation of its paragraph. Paragraphs that contain several
requirements may have additional untested or unimplemented cases.

## Guest corpus

The toolchain milestone selects the three-producer corpus and six C11 cases on both targets. ARM64 guest acceptance remains pending.
The corpus grew to 30 inputs with alignment, atomics, compound literals,
designated initializers and generic selection.

| Three-producer input | N1570 clauses exercised |
| --- | --- |
| [designated-init.c](../corpus/designated-init.c) | §6.7.9p17–21, array designators and initialization of omitted elements to 0. |
| [compound-literal.c](../corpus/compound-literal.c) | §6.5.2.5p4–6 and §6.7.9, a mutable, addressable block-scope compound literal with member designators. |
| [generic.c](../corpus/generic.c) | §6.5.1.1p3–4, compatible-type selection and an unevaluated controlling expression. |
| [alignment.c](../corpus/alignment.c) | §§6.5.3.4, 6.7.5, 6.7.10, 7.15 and 7.19, member and object alignment, member offset and successful static assertions. |
| [floating-conversion.c](../corpus/floating-conversion.c) | §§6.3.1.4 and 6.5.5, floating division and floating-to-integer conversion that discards the fractional part. |
| [atomics.c](../corpus/atomics.c) | §§7.17.2–3 and 7.17.7–8, initialization, fetch, compare-exchange, load and flag operations. |

| C11 guest case | N1570 clauses exercised |
| --- | --- |
| [declarations-initializers](../corpus/c11/declarations-initializers.c) | §§6.5.2.5p5, 6.7.2.1p18–20 and 6.7.9p17–21, compound literals, flexible members and designated initializers. |
| [generic-static-assert-alignment](../corpus/c11/generic-static-assert-alignment.c) | §§6.5.1.1, 6.5.2.2p6, 6.7.2.1p13, 6.7.4, 6.7.5, 6.7.10 and 6.9.1, generic selection, anonymous members, function specifiers, alignment, static assertions and identifier-list definitions. |
| [floating](../corpus/c11/floating.c) | §§5.2.4.2.2, 7.6, F.3, F.7 and F.8.4, dynamic rounding, conversions and exception flags. |
| [atomics](../corpus/c11/atomics.c) | §§5.1.2.4, 7.17.3 and 7.17.7.4, release/acquire message passing, sequentially consistent store buffering and contended aggregate compare-exchange. |
| [constraint-diagnostics](../corpus/c11/constraints.json) | The constraint paragraphs in the table below. |
| [profile-macros](../corpus/c11/profile-macros.c) | §6.10.8.3p1–2, conditional feature macros. |

On x86, release stores and acquire loads use the same instructions as relaxed
stores and loads. The message-passing case exercises the thread protocol.
The host assembly check independently requires the locked exchange for a
sequentially consistent store.

The constraint manifest lists the selected compile-fail fixtures under `guest`,
one per feature area.

| Feature area | Selected N1570 Constraints paragraph |
| --- | --- |
| Lexical elements | §6.4.3p2, restricted universal character name |
| Preprocessing | §6.10.3p6, duplicate macro parameter |
| Expressions | §6.5.16.2p1, pointer used as the right operand of integer compound addition |
| Generic selection | §6.5.1.1p2, duplicate compatible associations |
| Initializers | §6.7.9p2, out-of-bounds array designator |
| Declarations | §6.7.2.1p3, flexible array member before the last member |
| Alignment | §6.7.5p4, alignment weaker than the type's requirement |
| Atomics | §6.7.2.4p3, qualified type in an atomic type specifier |
| Static assertions | §6.7.10p2, false assertion |
| Control flow | §6.8.4.2p3, duplicate case value |

[profile-macros.c](../corpus/c11/profile-macros.c) exercises §6.10.8.3p1–2.
`__STDC_NO_VLA__`, `__STDC_NO_COMPLEX__` and `__STDC_NO_THREADS__` expand to
`1`. Both profiles leave `__STDC_NO_ATOMICS__` undefined and implement
scalar and aggregate atomics with [stdatomic.h](../include/stdatomic.h).
AArch64 uses native instructions for naturally aligned 1-, 2-, 4- and
8-byte objects. Wider and misaligned accesses use the generic size-first
`libbrt.a` calls. The AArch64 host harness runs the bcc-compiled,
bld-linked aggregate compare-exchange fixture from eight threads.
The profile also leaves `__STDC_IEC_559_COMPLEX__` undefined.

## Host constraints and open coverage

The full manifest is host-only. Each fixture is sent through bcc's
`-S -o` entry point for both target profiles. The check requires exit
status 1, empty stdout, the exact single stderr line and no output file.
The locations use paths relative to [the C11 corpus](../corpus/c11/).

| N1570 Constraints paragraph | Open coverage |
| --- | --- |
| §6.4.5p2 | Mixed wide and UTF-8 string literals. The current decoder only accepts ordinary strings and does not check the mixed-prefix rule. |
| §6.6p4 | Range checking of evaluated constant expressions, including signed arithmetic overflow. |
| §6.7p2 | Declarations with no declarator, tag or enumerator, such as `int;`. |
| §6.7.3p2 | Restrict qualification on types other than pointers to object types. |
| §6.7.4p3–4 | Restrictions on external inline definitions and function specifiers on hosted `main`. |
| §§6.7.6.2p2, 6.7.8p2, 6.8.4.2p2 | Scope, storage and switch-label rules for variably modified types await VLA support. |
| §6.9.1p2, p4 | Dedicated diagnostics for a definition with a nonfunction declarator or an illegal function storage class. |
| §6.10p5 | Whitespace enforcement within a preprocessing directive. |

Section 6.7.2.4p2 applies to a profile without atomic types. Sections
6.10.3p1–2 define replacement-list identity together with the
macro-redefinition rule. The manifest exercises the whitespace comparison
and the different-token case separately.

Sections 6.2.5 and 7.3 (complex arithmetic and `complex.h`) and §7.26
(C threads) remain open for bcc. The bundle copies Breenix libc's
[threads.h](../../libs/libbreenix-libc/include/threads.h) into its libc
header root. bcc currently fails to compile its dependency chain at the
`__SIZE_TYPE__` typedef in
[__breenix/types.h](../../libs/libbreenix-libc/include/__breenix/types.h).
The conditional threads macro describes support for using the header
with the compiler in this profile.

The ARM64 floating fixture uses binary128 precision and the same expected output as x86. ARM64 guest execution and acceptance remain open; the host results do not establish guest coverage.
