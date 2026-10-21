# Applications awaiting GPU window ports

B17 omits these programs from the effort image while `Window::framebuf` is removed.
The build reads this table to select Cargo binaries and installed ELFs. The gpu-apps
lane removes a row after its GPU port (or the planned demo retirement) is complete.
The pending list is part of userspace provenance, so changing it requires rebuilding
before an image can use prebuilt binaries. This temporary effort state cannot land on
main; gpu.md section 2.7 requires gpu-apps completion first.

| Program | Tracking issue | Remaining consumer |
|---|---|---|
| `bterm` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window text and terminal rendering |
| `bcheck` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window text and widgets |
| `bfontpicker` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window font preview and widgets |
| `blog` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window log rendering |
| `blauncher` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window text and selection |
| `bounce` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window sphere rendering |
| `rectangles` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window drawing; planned demo retirement |
| `biconkit` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window icons; planned demo retirement |
| `guskit` | [#2207](https://github.com/ryanbreen/breenix/issues/2207) | Window canvas; planned demo retirement |

The retained `text/fontwatch-idle` suite case is outside this application list.
It acquires a GPU Surface and explicitly fails naming gpu-text until that lane adds
GPU TrueType drawing. Its first-commit capture, independent pixel comparison and
clock assertions remain in place. It does not dereference a target handle or upload
CPU-rendered text. The GPU text work is tracked by
[#2206](https://github.com/ryanbreen/breenix/pull/2206).
