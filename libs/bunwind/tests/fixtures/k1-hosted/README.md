# K1 hosted link probes

These sections come from the retained K1 u1 `hosted_link_probe` ELFs reported
at `088b17c9`. K1 linked them using `linker-hosted.ld` and `--eh-frame-hdr`.

| ELF | SHA-256 | `.eh_frame` address | `.eh_frame_hdr` address |
|---|---|---|---|
| `hosted-x86_64.elf` | `bd9f3403a9fd5757d9d2223cb50ca8b59e567ce774db11876ecca3ece0b12b19` | `0x4001d248` | `0x4001d230` |
| `hosted-aarch64.elf` | `0c888f42d724b181cd78b5a4747ff1671a77ff38ae18b26c4cfd7d8e31e4027d` | `0x4001e190` | `0x4001e17c` |

Homebrew LLVM 20.1.8 commands, run for each `arch` in the directory with the ELFs:

```sh
llvm-objcopy --dump-section ".eh_frame=$arch.eh_frame" \
  --dump-section ".eh_frame_hdr=$arch.eh_frame_hdr" "hosted-$arch.elf" /dev/null
llvm-readelf -WS "hosted-$arch.elf"
llvm-dwarfdump --eh-frame "hosted-$arch.elf" > "$arch.dwarfdump"
```

The `.dwarfdump` files record the expected half-open FDE ranges and the CIE
initial instructions. DWARF 5 Table 7.29 supplies the instruction byte encodings.
