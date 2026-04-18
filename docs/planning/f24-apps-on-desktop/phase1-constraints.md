# F24 Phase 1 Constraints — GUI Apps On bwm Desktop

## Current ARM64 Launch Path

On `main`, `userspace/programs/src/init.rs` bypasses `bsh` on `aarch64` and directly spawns a minimal service list:

1. `/bin/bwm`
2. `/sbin/telnetd`
3. `/bin/bsshd` after `[init] Boot script completed`

This ordering is deliberate. F23 moved `bwm` before `telnetd` because the 75-second Parallels capture happened while init was still in the `telnetd` service path, leaving only the kernel cornflower-blue VirGL proof clear visible.

## F22 Guardrails

F22 found that early ARM64 userspace on Parallels is sensitive to AHCI-backed filesystem reads during the single-CPU boot window:

- Loading the large `bsh` ELF early can stall before `/etc/init.js` runs.
- bwm's first visible frame must avoid filesystem-backed font and hotkey config reads, so ARM64 bwm uses bitmap fonts and built-in hotkeys until after first presentation.
- bwm's normal op16 per-window VirGL compositor path times out on ARM64 Parallels; the working path is the op10 direct VirGL blit used by `graphics::virgl_composite`.
- `bsshd` is intentionally started after the boot script to avoid overlapping early exec reads against the AHCI-backed ext2 root.

The practical constraint for F24 is that new apps should be launched one at a time after bwm has been spawned, with a small pacing delay between spawns, and every increment must be validated from a fresh build/capture rather than inferred from process creation.

## bwm Multi-Window Behavior

bwm discovers client windows from the kernel window registry via `graphics::list_windows`, assigns cascade positions, sets kernel window positions, draws bwm chrome into its compositor buffer, and routes input through the window input queue.

On non-ARM64 targets, bwm uses `graphics::virgl_composite_windows_rect`, where the kernel uploads client window textures and composites content with the bwm chrome. On ARM64, F22 intentionally uses `graphics::virgl_composite(composite_buf, ...)` instead. That path only presents the CPU-composited buffer, so without an ARM64 fallback bwm can show the desktop and window frames but not client pixels.

## App ELF Sizes

Current aarch64 app binary sizes:

| App | Size | Notes |
| --- | ---: | --- |
| `bounce.elf` | 387,456 bytes | Smallest final-target GUI app; creates one Breengel window and animates locally. |
| `bcheck.elf` | 421,760 bytes | Optional diagnostic app. |
| `blog.elf` | 467,392 bytes | Opens log files and creates one Breengel window. |
| `bterm.elf` | 476,088 bytes | Largest target app; also spawns child PTY processes (`btop` and shell). |

F24 will start with `bounce` because the task asks to pick the smallest binary first. `bterm` remains the most useful app and should be attempted after smaller GUI clients prove the launch/content path.
