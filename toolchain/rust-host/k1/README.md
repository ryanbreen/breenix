# K1 compiler helper inventory

The records cover the 30 C objects named by `toolchain/corpus/cases`.
They come from the sealed 25.2 reference invocation retained in the
E25 ruling 66 bundles. Each row carries the object SHA-256 and the full
sorted list from `llvm-nm -u`. The records identify the original seals and
the Clang 22.1.8 and lld 20.1.8 producer digests. The assembler and linker
fixtures staged alongside this corpus have a separate producer contract.

To check a retained bundle or a reproduction from `scripts/build-toolchain.sh`,
pass its bundle directory and the guest runtime artifacts explicitly:

```sh
python3 toolchain/rust-host/k1/libc_host.py "$arch" "$bundle" "$libc" "$rlib" \
    --llvm-nm "$nm"
```

Use the C-ABI `libc.a` from the checkout under test and the target's
`libcompiler_builtins-*.rlib` from the verified, pinned Rust library build
in `userspace/programs/build.sh`. Use the pinned Rust toolchain's `llvm-nm`
so it can read the rlib's bitcode. The check verifies the bundle manifest
and per-object receipts, reruns the undefined-symbol scan, then subtracts
external definitions in `libc.a` and the rlib. X1 and X2 can add their own
records once their respective cross-links exist.
