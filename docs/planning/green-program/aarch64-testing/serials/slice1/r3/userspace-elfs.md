# The userspace ELFs the `boot_tests` profile links in — provenance for round 3

`kernel/src` `include_bytes!`s userspace ELF binaries that are gitignored, so a
fresh worktree does not carry them and the `--features boot_tests` build cannot
link without them. They were **not built in this worktree this round**. They
were copied, once, from the primary working copy at
`/Users/wrb/fun/code/breenix/userspace/programs/aarch64/`:

```
$ mkdir -p userspace/programs/aarch64
$ cp /Users/wrb/fun/code/breenix/userspace/programs/aarch64/*.elf userspace/programs/aarch64/
$ ls userspace/programs/aarch64/*.elf | wc -l
     152
```

Both directories hold 152 of 152 `.elf` files with the same contents. Checked by
hashing each file, stripping the directory prefix, sorting, and hashing the
resulting list, so the comparison covers names and bytes together:

```
$ shasum -a 256 userspace/programs/aarch64/*.elf | sed 's|.*/||' | sort | shasum -a 256
3016f1c9efea79a518e56255381d1e527344e633b44a455009b34aa62d847ee0  -

$ shasum -a 256 /Users/wrb/fun/code/breenix/userspace/programs/aarch64/*.elf | sed 's|.*/||' | sort | shasum -a 256
3016f1c9efea79a518e56255381d1e527344e633b44a455009b34aa62d847ee0  -
```

This branch changes no userspace source, so the copy is the right provenance
rather than a shortcut around one: rebuilding would have produced binaries for
sources this branch did not touch.

Sibling artifacts in this directory record the three aarch64 kernel builds that
consumed them — `aarch64-no-features-build.txt`,
`aarch64-boot_tests-build.txt` and `aarch64-testing-build.txt` — each with the
full `cargo` command on its second line and a `BUILD_EXIT=` line at the end.
claim-lint:ok: 3 of 3 build artifacts carry both, and 3 of 3 report
`BUILD_EXIT=0`.
