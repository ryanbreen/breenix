# Triangle stage negative controls

`constant-colour-fragment.tgsi` retains the diagnostic varying declaration but
writes literal RGBA (0.25, 0.5, 0.75, 1.0). `drop-colour-vertex.tgsi` retains the
diagnostic declarations but omits `MOV OUT[1], IN[1]`. Their committed `.bin`,
`.sph`, `.json` and `.asm` files are compiler outputs, not hand-edited mutations.
`provenance.json` records the source/output hashes, pinned source inventory,
recipe hashes, host executable hashes and toolchain versions used to make them.

The tools are the retained first build from U2's `kepler-assets --check` run.
The source inventory was verified before compiling these controls. To reproduce
using tools from a pinned asset build on a supported host:

```sh
export CARGO_BUILD_JOBS=4
python3 tools/kepler/assets.py --verify-sources
tools=target/kepler/assets/..
controls=tests/fixtures/gt710_shader_controls
for name in constant-colour-fragment drop-colour-vertex; do
    taskpolicy -b "$tools/kepler-compile" "$controls/$name.tgsi" "$controls/$name"
    taskpolicy -b "$tools/envydis" -m gk110 -i -n "$controls/$name.bin" > "$controls/$name.asm"
done
```

`taskpolicy -b` applies on macOS; omit that prefix on Linux. Compilation uses
the existing 7.1 compiler and decoder; it does not rebuild kernel assets.
Host executable hashes identify this generation run and can vary on another
host. Source pins and output bytes identify the controls.

Run the check on any Python build host without those tools:

```sh
python3 tests/gt710_shader_stage_test.py
```

It must accept the committed diagnostic pair, reject the constant fragment at
the target-0 dataflow assertion, and reject the vertex control at the missing
colour-output assertion. SPHs are read using the same header parser as U2.
The assembly checker admits a straight-line instruction slice; it is not a
GK110 emulator and does not establish rendered pixel or hardware execution
results.

The control definitions and generated shader code are Breenix code under the
root PolyForm Noncommercial 1.0.0 license. The SPH generation and host tools
retain their MIT notices in `tools/kepler/compile.cpp` and `vendor/kepler/NOTICES`;
the root `THIRD_PARTY_NOTICES` lists the upstream sources and consumers.
