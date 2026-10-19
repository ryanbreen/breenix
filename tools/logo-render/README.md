# Logo preview renderer

This host tool renders the proposed Breenix splash composition at scales 2, 3
and 4. Its only image encoding dependency is `png`; drawing comes from the
integer `breenix-logo` crate. It has a separate Cargo workspace so previewing
art does not build the kernel. No font files or external artwork are used.

From the repository root:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo run --release --manifest-path tools/logo-render/Cargo.toml -- docs/images/logo
```

An optional second directory receives copies for visual review:

```sh
logo-render OUTPUT_DIRECTORY REVIEW_DIRECTORY
```

The PNGs show a lit cursor, partial progress and done/running/pending dots.
They are generated documentation, intended for visual approval, with no
snapshot comparison test. The stage name and boot-mode text belong to the
console renderer rather than this artwork.

![Proposed Breenix splash at scale 2](../../docs/images/logo/breenix-logo-k2.png)
