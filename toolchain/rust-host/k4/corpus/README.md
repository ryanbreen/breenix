# K4 LLVM IR corpus

These inputs are written from the [LLVM Language Reference](https://llvm.org/docs/LangRef.html):
integer operations, SSA `phi` and branches, structure addressing, calls and
global constants. `hello.ll` calls `puts` with `Hello from llc!`; its expected
stdout is `Hello from llc!\n` and its expected exit status is 0.

`expect.json` records each complete llc argument vector, with `<input>` and
`<output>` replaced by the input and output paths. The explicit source filenames
keep the object identity independent of the directory containing the corpus.
The producer is the Mac reference llc in the separate `native-llc` directory.
Ruling 100 limits its target list to X86 and AArch64 because the Mac SDK headers
reject a Hexagon backend file; the box build retains the full target list. Both
use the same pinned source. `BREENIX_MAC_LLC` can select the recorded executable
at another path for the host check.
