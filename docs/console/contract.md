# The screen console contract

This is the contract of the `console` effort (`docs/efforts/path.json`): the boot logo, the text console and
its six virtual terminals on the firmware or boot framebuffer of each target. Lanes build to it, and the
`console` suite (`docs/suites/console.json`) measures it. A lane that needs a decision this document does not
make records it here in the same pull request as its code.

**Sources.** The contract is written from published specifications and documentation only: the UEFI Graphics
Output Protocol, the Intel SDM (PAT), the virtio specification (GPU 2D, input), POSIX.1-2024 (terminals),
`console_ioctl(2)`, `vcs(4)`, `chvt(1)`, `keymaps(5)`, and the device numbers in the published `devices.txt`.
Ioctl numbers, structure layouts and input key codes are interface facts taken from the published headers
(`linux/vt.h`, `linux/kd.h`, `input-event-codes.h`). Where those documents are silent, this contract chooses
and says so; it does not claim to behave as another system does. Lanes do not read Linux, BSD or vendor
source, and they do not open `kernel/src/drivers/usb/` (#1743) or `kernel/src/drivers/vmware/svga3.rs`
(#1770).

**Lanes named here:** V1 console core, V2a logo art, V2b splash, V3 emulation, V4 virtual terminals (kernel),
V4u virtual terminals (userspace), V5 input core and keys, V6 host keys, V7 ARM64 USB keyboard. A lane named
beside a rule owns its implementation.

---

## 1. The virtual terminal model

### 1.1 Terminals and devices

There are six virtual terminals, `tty1` to `tty6` (`VT_COUNT = 6`, one constant). Each is a terminal device
with its own line discipline, termios, session, foreground process group, blocked-reader wait queue, screen
grid, cursor, attributes, scrollback, keyboard mode state (Caps Lock), KD mode and VT mode (section 4).

| Node | Device number (major, minor) | What it is |
|---|---|---|
| `/dev/tty0` | 4, 0 | The foreground terminal at the time of each operation: writes, reads and ioctls go to whichever VT is in the foreground then |
| `/dev/tty1` .. `/dev/tty6` | 4, 1 .. 4, 6 | The six terminals |
| `/dev/console` | 5, 1 | tty1: the same device as `/dev/tty1` |
| `/dev/tty` | 5, 0 | The caller's controlling terminal: a VT, tty1 through `/dev/console`, or a pseudo-terminal; ENXIO without one |
| `/dev/vcs`, `/dev/vcsa` | 7, 0 and 7, 128 | The foreground VT's screen, resolved at each `read` (section 3) |
| `/dev/vcs1` .. `/dev/vcs6` | 7, 1 .. 7, 6 | VT *n*'s screen text |
| `/dev/vcsa1` .. `/dev/vcsa6` | 7, 129 .. 7, 134 | VT *n*'s screen text with attributes |

`/dev/tty` already resolves the controlling terminal through `syscall/fs.rs::handle_devfs_open` and
`tty/ctty.rs`. V4 extends that resolution from `TtyId::Console` to a numbered VT (`TtyId::Vt(n)`), keeps its
pseudo-terminal and ENXIO behaviour, and moves the console-specific job-control branches to apply to each VT.

The terminal nodes keep the owner and mode `/dev/console` has today. The `vcs` nodes are owned by root with
mode 0600.

### 1.2 tty1 is the console

- tty1 is the device that is `/dev/console` today (TTY 0 in `tty/driver.rs`). It has two sinks, serial and
  the screen, and two sources, serial and the keyboard. tty2 to tty6 have the screen and the keyboard only.
- **Serial input goes to tty1** whichever VT is in the foreground. **Keyboard input goes to the foreground
  VT.**
- tty1's output goes to serial regardless of which VT is in the foreground, and into tty1's grid. Output of
  tty2 to tty6 does not go to serial.
- The `shell`, `console` and `default` boot modes and C4's getty keep using this device.

### 1.3 The kernel log on the screen

The kernel log is drawn on tty1 only, under C1's policy: records at INFO and above while tty1 belongs to
init's session or to no session, then warnings and errors only once a getty or login session holds tty1.
Serial keeps each record. Records written while another VT is in the foreground go into tty1's grid and
scrollback and are not drawn over the foreground VT. While the splash is up, records go into tty1's grid, so
Shift+PageUp after the hand-over shows the boot log.

### 1.4 Terminals in the background

A VT that is not in the foreground is a terminal like any other; only its screen is not shown.

- Its processes keep running and are scheduled as before.
- Writes to it complete as they would in the foreground: the output goes into its grid and scrollback and is
  on screen when the VT returns. Visibility does not make a write block or fail.
- A read on it waits for input as on any terminal. Keyboard input reaches it only while it is in the
  foreground; a blocked read stays blocked across switches and completes when input arrives.
- Switching sends no signal to any process (no `SIGTTOU`, `SIGTTIN`, `SIGSTOP`, `SIGCONT` or `SIGWINCH`), and
  leaves each VT's session and foreground process group unchanged. Job control on each VT is unchanged by
  visibility.
- The suite cases `vt-devices/background-*` and `vt-devices/switch-sends-no-signal` measure these points (V4).

### 1.5 Sizes

Each VT has the grid of the size rule (section 2) for the display, so `TIOCGWINSZ` reports it on each VT:
`ws_row` and `ws_col` are the grid, `ws_xpixel` and `ws_ypixel` the grid's pixel size (columns times cell
width, rows times cell height). The size is fixed for the boot, so no `SIGWINCH` is sent. `TIOCSWINSZ` with
the VT's own size succeeds without effect; with any other size it fails with EINVAL.

### 1.6 Scrollback

- 1,000 lines per VT, allocated when the VT first receives output. If the allocation fails, the VT runs
  without scrollback and the kernel logs `[console] ttyN scrollback unavailable` once.
- **Shift+PageUp** and **Shift+PageDown** move the foreground VT's view back and forward by half a screen
  (`rows / 2`, rounded down): the `Scroll_Backward` and `Scroll_Forward` actions `keymaps(5)` names.
- New output does not move a scrolled-back view. A key that sends input to the VT returns the view to the
  live screen. The cursor is not drawn while the view is scrolled back.
- Each VT's scrollback and view position survive switches.
- The alternate screen (section 5.4) has no scrollback: lines scrolled off it are discarded, and Shift+PageUp
  has no effect while it is active.

---

## 2. The size rule and the screen layout

**The font size `S` is the largest of 16, 20, 24 and 32 whose grid is at least 128 columns by 36 rows.** The
font is `noto-sans-mono-bitmap` (SIL OFL), regular and bold weights. A cell is the glyph width by `S + 2`
pixels; the two extra pixel rows at the bottom of each cell are the cursor band. The glyph widths are 7, 9,
11 and 14 pixels for sizes 16, 20, 24 and 32.

- `columns = min(floor(width / cell_width), 255)` and `rows = min(floor(height / cell_height), 255)`. The cap
  keeps the `vcsa` header exact (section 3); pixels past the grid are margin.
- A display too small for 128 by 36 at size 16 (under 896 by 648) uses size 16 with whatever grid fits.
- The grid's origin is the framebuffer's top-left pixel; the margin is on the right and at the bottom, in the
  background colour.
- The logo scale `k` (pixels per design unit, section 6) follows `S`: 2 for sizes 16 and 20, 3 for 24, 4 for
  32.

| Display | Where | `S` | Cell | Grid | `k` |
|---|---|---|---|---|---|
| 1024x768 | x86-metal GOP | 16 | 7x18 | 146x42 | 2 |
| 1280x800 | ARM64 QEMU default | 20 | 9x22 | 142x36 | 2 |
| 1280x960 | Parallels and VMware defaults | 20 | 9x22 | 142x43 | 2 |
| 1920x1080 | x86-64 QEMU (`virtio-vga`) | 24 | 11x26 | 174x41 | 3 |
| 1920x1200 | VMware `--retina` | 24 | 11x26 | 174x46 | 3 |
| 2560x1600 | Parallels `--retina`, GL QEMU | 32 | 14x34 | 182x47 | 4 |

The console enables the font crate's `size_20`, `size_24`, `size_32`, `bold`, `unicode-latin-1-supplement`
and `unicode-latin-extended-a` features (V1).

---

## 3. The `vcs` and `vcsa` projection

The cells hold more than `vcs(4)`'s two-byte words carry. This is how a VT's screen is projected onto those
devices; tests read cells through them, and the kernel's pixel read-back (section 8) stays the kernel's own
check.

- **What is projected:** the VT's live screen as displayed: the alternate screen while it is active, and the
  live screen even while the view is scrolled back. Scrollback is not projected.
- **`/dev/vcsN`:** one byte per cell, rows top to bottom, each row left to right, with no line separators.
  Its size is `rows * columns` bytes.
- **`/dev/vcsaN`:** a four-byte header, then two bytes per cell in the same order. Its size is
  `4 + 2 * rows * columns` bytes.
  - Header bytes, in order: rows, columns, cursor column, cursor row (0-based). Each fits in one byte because
    the size rule caps the grid at 255 by 255 (section 2).
  - Each cell is a little-endian 16-bit word: the low byte is the character byte, the high byte is the
    attribute byte.
- **The character byte** is the cell's character when its code point is U+0000 to U+00FF (Latin-1 maps to
  itself), and `?` (0x3F) for any other character, including Latin Extended-A and box drawing. A blank cell
  is a space (0x20).
- **The attribute byte** is the colour pair as displayed: bits 0-3 are the foreground palette index and bits
  4-7 the background palette index (0-15, section 5.2), after reverse video is applied. A cell drawn
  invisible (SGR 8) has its foreground index equal to its background index. Bold and underline are not
  represented. This layout is Breenix's: the palette order is the SGR colour order, not a hardware palette's.
- **Reads:** each `read` copies from one consistent snapshot of the grid, taken under the console lock at the
  time of that call. `lseek` with `SEEK_SET`, `SEEK_CUR` and `SEEK_END` works against the size above; a read
  at or past the end returns 0.
- **Writes** fail with EINVAL: the devices are read-only in this effort.
- `/dev/vcs` and `/dev/vcsa` resolve the foreground VT at each `read`.

---

## 4. Ioctls

### 4.1 The requests

Numbers and layouts are from the published `linux/vt.h` and `linux/kd.h`. They work on a descriptor for
`/dev/tty1` .. `/dev/tty6`, `/dev/tty0`, `/dev/console`, or `/dev/tty` when the controlling terminal is a VT.
On a pseudo-terminal or any other device they fail with ENOTTY. "The descriptor's VT" means VT *n* for
`/dev/ttyN`, tty1 for `/dev/console`, and the foreground VT at the time of the call for `/dev/tty0`.

| Request | Number | Argument | Does | Errors |
|---|---|---|---|---|
| `VT_OPENQRY` | 0x5600 | `int *` | Writes the lowest VT number that no process has open, or -1 when the six are open | EFAULT |
| `VT_GETMODE` | 0x5601 | `struct vt_mode *` | Writes the descriptor's VT's mode | EFAULT |
| `VT_SETMODE` | 0x5602 | `const struct vt_mode *` | Sets the descriptor's VT's mode (section 4.4) | EFAULT, EINVAL, EPERM, EBUSY |
| `VT_GETSTATE` | 0x5603 | `struct vt_stat *` | Writes the foreground VT and the open VTs | EFAULT |
| `VT_RELDISP` | 0x5605 | value | Answers a release or acquisition (section 4.4) | EINVAL, EPERM |
| `VT_ACTIVATE` | 0x5606 | value: VT number | Requests a switch to that VT and returns (section 4.4) | ENXIO, EPERM, EBUSY |
| `VT_WAITACTIVE` | 0x5607 | value: VT number | Blocks until that VT is active (section 4.5) | ENXIO, EINTR |
| `KDSETMODE` | 0x4B3A | value: `KD_TEXT` 0 or `KD_GRAPHICS` 1 | Sets the descriptor's VT's KD mode (section 4.3) | EINVAL, EPERM, EBUSY |
| `KDGETMODE` | 0x4B3B | `int *` | Writes the descriptor's VT's KD mode | EFAULT |

- **Layouts.** `struct vt_mode` is 8 bytes: `mode` (u8, offset 0: `VT_AUTO` 0, `VT_PROCESS` 1), `waitv` (u8,
  offset 1), `relsig` (i16, offset 2), `acqsig` (i16, offset 4), `frsig` (i16, offset 6). `struct vt_stat`
  is 6 bytes: `v_active`, `v_signal`, `v_state` (u16 each, offsets 0, 2, 4). `VT_ACKACQ` is 2.
- **`VT_GETSTATE`:** `v_active` is the foreground VT (during a pending switch, the VT still in the
  foreground); `v_signal` is 0; bit *n* of `v_state` is set while VT *n* is open, and bit 0 is clear. A VT is
  open while a process holds a descriptor for `/dev/ttyN`, for `/dev/console` (tty1), or for `/dev/tty`
  resolved to it. Descriptors for `/dev/tty0` and the `vcs` devices do not count.
- **Permission.** The requests that change state (`VT_SETMODE`, `VT_ACTIVATE`, `KDSETMODE`) need root, or a
  controlling terminal that is a VT; `VT_SETMODE` and `KDSETMODE` without root also need that controlling
  terminal to be the descriptor's VT. Otherwise they fail with EPERM. `VT_RELDISP` is answered only by the
  VT's `VT_PROCESS` owner (EPERM for any other process, root included). The read-only requests and
  `VT_WAITACTIVE` need only the descriptor.
- **Numbers outside 1-6.** `VT_ACTIVATE` and `VT_WAITACTIVE` fail with ENXIO for a VT number outside 1 to 6.
  `console_ioctl(2)` lists EINVAL for an invalid argument in general; Breenix answers ENXIO because the
  argument is well formed and names a terminal that does not exist, the meaning ENXIO has for `open`.
- **Other VT and KD requests** (`VT_DISALLOCATE`, `VT_RESIZE`, `KDGKBMODE`, `KDSKBMODE` and the rest of those
  two headers) fail with ENOTTY in this effort.

### 4.2 Display ownership

The **display owner** is the process holding the screen through `sys_take_over_display`, or, on `--gpu-gl`
boots once the GPU effort's console draws the VTs, the top of the GPU display ownership stack
(`kernel/src/gpu/display.rs`).

- Taking the display puts the owner's VT into `KD_GRAPHICS` with the owner as its graphics owner. The owner's
  VT is its controlling terminal when that is a VT, else tty1. Taking the display is refused with EBUSY while
  the owner's VT is not the foreground.
- **Switching away from the display owner's VT is refused with EBUSY unless the owner chose `VT_PROCESS`**,
  so a desktop or a suite panel is not left half-switched. With `VT_PROCESS`, the release protocol of
  section 4.4 applies.
- **After its VT is released, the owner keeps its ownership but loses presentation authority:** what it
  draws, presents or flushes does not reach the screen, and a direct framebuffer mapping it holds no longer writes
  to the displayed pixels. Its calls succeed; their effect is not shown. When its VT is acquired again the
  owner's authority returns, the kernel sends `acqsig`, and the owner redraws: the kernel does not restore
  its pixels. On `--gpu-gl` boots the GPU effort implements this suspension of its ownership stack and the
  restoration of the owner's mode and target (Q8); V4 implements it for the framebuffer path.
- A panic or fatal fault takes the screen regardless of VT state.

### 4.3 KD mode

- `KD_TEXT`: the kernel draws the VT from its grid while it is in the foreground. `KD_GRAPHICS`: the kernel
  does not draw the VT; output to it still goes into its grid and scrollback, and the screen belongs to its
  graphics owner while it is in the foreground.
- `KDSETMODE(KD_GRAPHICS)` makes the caller the VT's graphics owner. `KDSETMODE(KD_TEXT)` redraws the VT if it
  is in the foreground. Any other value fails with EINVAL.
- `KDSETMODE` on the display owner's VT by any other process fails with EBUSY: that VT stays in
  `KD_GRAPHICS` until its owner gives the display back or exits.
- **When the graphics owner exits** (normally or by a signal), its VT returns to `KD_TEXT` and `VT_AUTO`, the
  display is released if it held it, the kernel redraws the VT if it is in the foreground, and the kernel logs
  `[console] ttyN owner gone: mode reset`.
- During the splash, tty1 reads `KD_GRAPHICS` with the kernel as its graphics owner, and any `KDSETMODE` on
  tty1 ends the splash (section 7).

### 4.4 Process-controlled switching (`VT_PROCESS`)

**Per VT:** a VT mode (`VT_AUTO` or `VT_PROCESS`), its owner, `relsig` and `acqsig`. **For the console:** the
foreground VT `fg`, whether its acquisition is complete, and at most one pending switch `{from, to, phase, source, deadline}`, where phase is
`releasing` or `acquiring`. The console lock protects both. Deadlines: `VT_RELEASE_TIMEOUT_MS = 3000` and
`VT_ACQUIRE_TIMEOUT_MS = 3000`.

**`VT_SETMODE`.**
- `VT_PROCESS` makes the caller the VT's owner. `relsig` and `acqsig` must be valid signal numbers; `frsig`
  and `waitv` are ignored. A `mode` other than 0 or 1, or an invalid signal, fails with EINVAL.
- When another live process already owns the VT in `VT_PROCESS`, the call fails with EBUSY.
- `VT_AUTO` from the owner returns the VT to automatic switching; from any other process it fails with EPERM.
- When the owner exits, the VT returns to `VT_AUTO` (and to `KD_TEXT` if the owner was also its graphics
  owner).

**A switch request** (a key, `VT_ACTIVATE` or `chvt`) to VT `to`:

1. Permission and range are checked; `VT_ACTIVATE` fails at once with EPERM or ENXIO.
2. **With no switch pending:**
   - `to == fg`: no switch is made; `VT_ACTIVATE` returns 0.
   - `fg` has a display owner in `VT_AUTO`: refused. `VT_ACTIVATE` fails with EBUSY; a key is consumed and the
     kernel logs `[console] switch to ttyN refused: ttyM display owned (<source>)`.
   - `fg` is `VT_AUTO`: the switch is made at once: `fg := to` (the acquiring step below), and `VT_ACTIVATE`
     returns 0.
   - `fg` is `VT_PROCESS`: the kernel records the pending switch in phase `releasing` with a deadline, sends
     `relsig` to `fg`'s owner, and `VT_ACTIVATE` returns 0. `fg` does not change yet.
3. **With a switch pending in `releasing`:**
   - `to` differs from the pending `from`: **the latest request wins**: the pending target becomes `to`, and
     `VT_ACTIVATE` returns 0. The owner is not signalled again.
   - `to == from`: **the pending switch is cancelled**: it is cleared, `fg` stays, the kernel sends `acqsig` to
     the owner so an owner that already stopped drawing resumes, and `VT_ACTIVATE` returns 0.
4. **With a switch pending in `acquiring`:** the request is held as the next switch (latest wins) and starts
   when the acquisition completes; `VT_ACTIVATE` returns 0.

**Answers from the owner of `from` (`VT_RELDISP`) in `releasing`:**
- `1` (release): the switch proceeds to the acquiring step.
- `0` (refuse): the pending switch is cleared and `fg` stays. The kernel logs
  `[console] switch to ttyN refused by ttyM owner`.
- **No answer by the deadline:** treated as a refusal, logged `[console] switch to ttyN cancelled: ttyM owner
  did not release`.
- **The owner exits:** treated as a release (its VT has been reset by then, section 4.3).
- `VT_RELDISP(0)` or `VT_RELDISP(1)` with no release pending (after a cancellation or a timeout) fails with
  EINVAL.

**The acquiring step**, under the console lock: mark acquisition incomplete and record phase `acquiring`
before setting `fg := to`; the pending `from` loses presentation authority if it has a display owner
(section 4.2); the render thread is woken at normal priority.
- `to` in `KD_TEXT`: the render thread redraws each row of `to` from its grid into the shadow, copies it to the
  screen (and, on virtio-gpu, transfers and flushes it). This redraw must finish before acquisition completes.
- A `VT_PROCESS` destination, in either KD mode, receives `acqsig` and an acquisition deadline. Its owner
  answers `VT_RELDISP(VT_ACKACQ)`; the answer, deadline or owner's exit satisfies the owner part of acquisition.
  For `KD_TEXT`, both the redraw and the owner part must finish; either may finish first. For `KD_GRAPHICS`,
  only the owner part is required: the kernel does not draw. `VT_ACKACQ` with no acquisition pending, or after
  the owner part has already finished, fails with EINVAL.
- `to` in `KD_GRAPHICS` with a `VT_AUTO` graphics owner: the acquisition is complete at once; the kernel does
  not draw. A `KD_TEXT` destination in `VT_AUTO` needs only the redraw.
- Once the required redraw and owner acknowledgement finish, under the console lock mark acquisition complete,
  clear the pending switch
  and wake `VT_WAITACTIVE` waiters. An owner's exit resets its mode as in section 4.3; if this makes the
  destination `KD_TEXT`, the resulting redraw must finish too.
- A held next switch then starts as a new request.

### 4.5 `VT_WAITACTIVE`

- `VT_WAITACTIVE(n)` returns 0 at once only when `n == fg` and its acquisition is complete (including while
  a switch away from `n` is pending in `releasing`). Testing this condition and registering a waiter use the
  console lock, so completion cannot race with going to sleep.
- Otherwise it blocks until a switch to `n` completes its acquisition (section 4.4): for a text VT, after the
  new screen has been copied to the display (and, on virtio-gpu, transferred and flushed); for any `VT_PROCESS`
  VT, also after `VT_ACKACQ`, the acquisition deadline or the owner's exit. Waiters are woken in that
  order, before the switch's read-back check runs (section 8).
- A refused or cancelled switch does not wake a waiter; it keeps waiting.
- A signal ends the wait with EINTR, or restarts it when the handler has `SA_RESTART`.
- The waiter blocks as `BlockedOnIO`, because a key-driven switch is requested from interrupt context and
  interrupt-context wakes move only `BlockedOnIO` threads.

---

## 5. Cells, colours and emulation (`TERM=breenix`)

### 5.1 Cells

A cell holds one character from the repertoire, a foreground and a background palette index (0-15), and the
attribute bits bold, underline, reverse and invisible. The repertoire is U+0020-U+007E, U+00A0-U+00FF,
U+0100-U+017F (from the font) and U+2500-U+257F (box drawing, generated geometrically at each cell size by
Breenix's own code). A malformed UTF-8 sequence and a character outside the repertoire are shown as one cell
of `?`. Characters are one cell wide.

- **Bold** draws the font's bold weight; it does not brighten the colour. **Reverse** swaps the pair as drawn.
  **Invisible** draws the background colour for the glyph. **Underline** fills the cell's first cursor-band
  pixel row in the foreground colour.
- **The cursor** fills the two cursor-band pixel rows of the cursor cell in the cell's displayed foreground
  colour. It does not blink, so a read-back sees one state.
- **Antialiasing:** a font sample `a` (0-255) blends the displayed foreground `fg` and background `bg`
  per channel as `(fg * a + bg * (255 - a) + 127) / 255`, integer division. Box-drawing strokes use
  binary coverage over the whole cell, including the cursor band, to join adjacent rows. Their arm weights,
  dash counts, rounded corners and diagonals are the geometry table in `kernel/src/console/boxdraw.rs` (V3).

### 5.2 Palette

The default foreground is 7 and the default background is 0. Colour 0 is the splash's field, so the console
continues the splash's background.

| Index | Name | RGB | Index | Name | RGB |
|---|---|---|---|---|---|
| 0 | black | 15, 20, 35 | 8 | bright black | 90, 100, 125 |
| 1 | red | 205, 70, 70 | 9 | bright red | 245, 110, 110 |
| 2 | green | 70, 200, 120 | 10 | bright green | 120, 230, 160 |
| 3 | yellow | 215, 180, 70 | 11 | bright yellow | 245, 215, 110 |
| 4 | blue | 70, 120, 215 | 12 | bright blue | 120, 165, 245 |
| 5 | magenta | 175, 95, 200 | 13 | bright magenta | 215, 140, 240 |
| 6 | cyan | 60, 175, 185 | 14 | bright cyan | 110, 215, 225 |
| 7 | white | 205, 215, 235 | 15 | bright white | 245, 248, 255 |

**Mapping 256-colour and 24-bit colours to the 16:** an index `n` below 16 is itself. For 16 to 231,
`n - 16 = 36r + 6g + b` and each component is `0` when 0, else `55 + 40 * component`. For 232 to 255, each
component is `8 + 10 * (n - 232)`. The RGB colour (from that, or given directly by `38;2;r;g;b`) maps to the
palette entry with the smallest squared distance `dr² + dg² + db²`, the lowest index on a tie.

### 5.3 SGR

| Parameter | Effect |
|---|---|
| 0 or empty | Reset attributes and colours to the defaults |
| 1 / 22 | Bold on / off |
| 4 / 24 | Underline on / off |
| 7 / 27 | Reverse on / off |
| 8 / 28 | Invisible on / off |
| 30-37, 90-97 | Foreground 0-7, 8-15 |
| 40-47, 100-107 | Background 0-7, 8-15 |
| 38;5;n, 48;5;n | Foreground, background: 256-colour index `n`, mapped (5.2) |
| 38;2;r;g;b, 48;2;r;g;b | Foreground, background: RGB, mapped (5.2) |
| 39 / 49 | Default foreground / background |

Other parameters are parsed and have no effect.

### 5.4 Control functions

| Sequence | Name | Effect |
|---|---|---|
| BS, HT, LF, VT, FF, CR | C0 | Back one column; next tab stop; line feed (VT and FF as LF); carriage return |
| BEL | | No effect |
| CAN, SUB | | Abort the sequence in progress |
| `ESC c` | RIS | Full reset: screen cleared, cursor home, attributes, tab stops, modes, scroll region and alternate screen reset |
| `ESC 7`, `ESC 8` | DECSC, DECRC | Save, restore cursor position and attributes |
| `ESC D`, `ESC E`, `ESC M` | IND, NEL, RI | Index, next line, reverse index (scrolling the region at its edge) |
| `ESC H` | HTS | Set a tab stop at the cursor column |
| `CSI n A/B/C/D` | CUU, CUD, CUF, CUB | Cursor up, down, forward, back |
| `CSI n E/F` | CNL, CPL | Cursor next, previous line |
| `CSI n G`, `CSI n d` | CHA, VPA | Cursor column, row |
| `CSI r ; c H`, `CSI r ; c f` | CUP, HVP | Cursor position (1-based) |
| `CSI n J` | ED | Erase in display: 0 below, 1 above, 2 the whole screen |
| `CSI n K` | EL | Erase in line: 0 right, 1 left, 2 the whole line |
| `CSI n L/M` | IL, DL | Insert, delete lines in the scroll region |
| `CSI n @/P/X` | ICH, DCH, ECH | Insert, delete, erase characters |
| `CSI n S/T` | SU, SD | Scroll up, down in the scroll region |
| `CSI n g` | TBC | 0 clears the stop at the cursor, 3 clears each stop |
| `CSI t ; b r` | DECSTBM | Scroll region (1-based, inclusive); the cursor moves home |
| `CSI s`, `CSI u` | | Save, restore cursor position |
| `CSI 5 n` | DSR | Answers `ESC [ 0 n` on the VT's input |
| `CSI 6 n` | DSR (CPR) | Answers `ESC [ row ; col R` (1-based) on the VT's input |
| `CSI ? 25 h/l` | DECTCEM | Cursor shown, hidden |
| `CSI ? 7 h/l` | DECAWM | Autowrap on (default), off |
| `CSI ? 1049 h/l` | | Alternate screen on, off, saving and restoring the cursor; the alternate screen starts cleared |
| `CSI m` | SGR | Section 5.3 |

- Parameters: up to 16 per sequence, the rest ignored; each value is clamped to 65535; a missing value takes
  the function's default (1 for movement and counts, 0 for selectors).
- Autowrap: printing in the last column leaves the cursor there with a wrap pending; the next printable
  character first moves to the start of the next line (scrolling the region at its bottom). A cursor movement
  clears the pending wrap. With autowrap off, characters past the last column overwrite it.
- Tab stops start at each 8th column. HT moves to the next stop, or to the last column.
- Sequences not listed are consumed without effect.
- Login and init set `TERM=breenix`. A terminfo source entry is added when a curses library arrives.

### 5.5 Keyboard input bytes

The console keymap (V5) is the US layout. Ctrl with a letter sends that letter's control code. Alt with a key
that has no console action sends `ESC` followed by the key's bytes.

| Key | Bytes | Key | Bytes |
|---|---|---|---|
| Up, Down, Right, Left | `ESC [ A`, `B`, `C`, `D` | F1-F4 | `ESC O P`, `Q`, `R`, `S` |
| Home, End | `ESC [ H`, `ESC [ F` | F5-F8 | `ESC [ 15 ~`, `17 ~`, `18 ~`, `19 ~` |
| Insert, Delete | `ESC [ 2 ~`, `ESC [ 3 ~` | F9-F12 | `ESC [ 20 ~`, `21 ~`, `23 ~`, `24 ~` |
| Page Up, Page Down | `ESC [ 5 ~`, `ESC [ 6 ~` | Backspace, Enter, Tab, Esc | 0x7F, CR, HT, ESC |

---

## 6. The boot logo

### 6.1 Geometry

The logo is defined in **design units**; at scale `k` a unit is `k` pixels. Shapes are filled with one colour,
except the badge's gradient. The geometry table in `libs/breenix-logo` (V2a) is this section as data, and the
operator reviews the rendered look; V2b may implement and draw the proposed design in the kernel while
that review is pending (F2). Approval does not block implementation. A change of art changes this section
and that table together.

**Colours.** Field (the screen's background) 15, 20, 35. Badge gradient from 52, 92, 190 (top) to 34, 150, 160
(bottom). The B 236, 241, 250. Cursor block 70, 200, 120. Wordmark 205, 215, 235. Bar track 40, 50, 75 and
fill 70, 200, 120. Running dot 230, 170, 65. Pending-dot outline 115, 135, 170. Dim text 110, 120, 145.

**Shape kinds.** Here `rect(x, y, w, h)` uses position and size, as does the crate's table; its closed box is
`x <= px <= x + w, y <= py <= y + h`. `rrect(x, y, w, h, r)` is that box with rounded corners: clamp the
point to the inner box `[x+r, x+w-r]` by `[y+r, y+h-r]` and include it when the squared distance to the
clamped point is at most `r²`. `half_annulus(cx, cy, R, r)` is `px >= cx` and
`r² <= (px - cx)² + (py - cy)² <= R²`. `quad(p0, p1, p2, p3)` is the convex quadrilateral with those
corners in order, its edges included. `disc(cx, cy, r)` is `(px - cx)² + (py - cy)² <= r²`, and
`ring(cx, cy, R, r)` is `r² <= d² <= R²`. Fractional coordinates below are exact: the table stores
integers in thirds of a design unit.

**The badge** (64 by 64 units, origin at its top-left):

| Layer | Shapes | Colour |
|---|---|---|
| Badge | `rrect(0, 0, 64, 64, 13)` | Gradient |
| B spine | `rect(16, 14, 6, 36)` | B |
| B upper bowl | `half_annulus(22, 23, 9, 4)` | B |
| B lower bowl | `half_annulus(22, 39, 11, 5)` | B |
| Cursor | `rect(38, 45, 13, 5)` | Cursor block |

**The wordmark "BREENIX"** has ink bounds x=19..157, y=80..104 in the composition: 138 by 24 units.
Its ink centre is x=88, on the badge/bar axis. B, R, E and N are 16 units wide; the I is 10 and
X is 18. Straight stems are 4 units wide. B and R share one upper bowl, using the badge's upper
bowl radii at two-thirds scale; B's lower bowl likewise uses the scaled badge radii. Straight
shoulders extend the bowls to the wordmark's proportions. The badge itself retains its geometry.
Letters below use local coordinates relative to their listed left edge and y=80, in table order:

| Letter | Left | Shapes |
|---|---|---|
| B | 19 | `rect(0, 0, 4, 24)`, `rect(0, 0, 26/3, 10/3)`, `half_annulus(26/3, 6, 6, 8/3)`, `rect(0, 26/3, 26/3, 14/3)`, `half_annulus(26/3, 50/3, 22/3, 10/3)`, `rect(0, 20, 26/3, 4)` |
| R | 42 | `rect(0, 0, 4, 24)`, `rect(0, 0, 26/3, 10/3)`, `half_annulus(26/3, 6, 6, 8/3)`, `rect(0, 26/3, 26/3, 10/3)`, `quad((4, 11), (9, 11), (16, 24), (11, 24))` |
| E | 64 | `rect(0, 0, 4, 24)`, `rect(0, 0, 16, 4)`, `rect(0, 10, 13, 4)`, `rect(0, 20, 16, 4)` |
| E | 84 | as above |
| N | 104 | `rect(0, 0, 4, 24)`, `quad((0, 0), (4, 0), (16, 24), (12, 24))`, `rect(12, 0, 4, 24)` |
| I | 126 | `rect(3, 0, 4, 24)`, `rect(0, 0, 10, 4)`, `rect(0, 20, 10, 4)` |
| X | 139 | `quad((0, 0), (5, 0), (18, 24), (13, 24))`, `quad((13, 0), (18, 0), (5, 24), (0, 24))` |

The bounding-box gaps B-R, R-E, E-E, E-N, N-I and I-X are 7, 6, 4, 4, 6 and 3 units.
These are optical gaps: the bowls recede, E has open sides, I's stem is inset, and X's diagonals
recede toward its waist. The spacing is judged from the rendered contours rather than a fixed pitch.

### 6.2 Layout on the screen

The crate draws a 176 by 136 unit composition. In pixels, with `W` by `H` the framebuffer,
`cx = floor(W / 2)` and `top = floor(2 * H / 5) - 32k` (the badge's centre at 40% of the height),
pass composition origin `(left, top) = (cx - 88k, top)` to `render`. Positions below are relative to that
origin unless given as screen coordinates:

| Element | Position and size |
|---|---|
| Badge | `(56k, 0)`, 64 by 64 units |
| Wordmark | local letter coordinates from 6.1, top `80k` |
| Progress bar | `(0, 114k)`, 176 by 4 units: track `rrect(0, 114, 176, 4, 2)` |
| Milestone dots | centre x `k * [3, 37, 71, 105, 139, 173][i]`, centre y `130k`, radius `3k` |
| Stage name | console font size `S`, dim, centred on screen `cx`, screen top `top + 141k` |
| Boot mode, status | console font size `S`, dim, centred on screen `cx`, tops `H - 3(S + 2)` and `H - 2(S + 2)` |

- **The bar's fill** has width `f = floor(176 * min(reached, total) / total)` design units; total 0 gives
  width 0. When `f > 0`, draw `rrect(0, 114, f, 4, min(floor(f / 2), 2))` in green over the track.
  Both layers have antialiased rounded edges. Width is quantized in design units before scaling by `k`.
  `reached` and `total` count the kernel stages of the six milestones below.
- **The dots** are the boot path's milestones `arch-bringup`, `interrupts-time`, `devices`, `filesystems`,
  `scheduler` and `first-userspace`, over their arch's stage lists in `docs/boot-path.json`, less stages
  printed after PID 1 starts. A dot is a fill `disc` in the fill colour when each of its stages is reached,
  in the running colour while some are, and a `ring(…, 3k, 2k)` in the pending colour while no stage of it is.
- **The stage name** is the latest stage reached. The boot-mode line reads `Boot mode: <mode>` and the status
  line names the next step, as today's boot screen does. Text is outside the crate's composition.
- **The cursor block blinks:** drawn during the first 500 ms of each second since the splash was first
  drawn, and not drawn (the badge shows) in the second 500 ms; held lit after boot hand-over.

### 6.3 Rasterization

Each pixel `(px, py)` of a shape's layer has a coverage `c` from 0 to 16: the number of its 16 sample points
inside the layer's shapes (a point inside any of them counts once). The sample points are at
`((8(px - left) + 2i + 1) / (8k), (8(py - top) + 2j + 1) / (8k))` in composition units,
for `i` and `j` from 0 to 3. Shape boundaries are included as specified in 6.1.

- **Layer grouping and order:** enumerate badge, badge B, wordmark, visible cursor, track, nonempty fill,
  and dots left to right. Adjacent shapes with the same resolved RGB colour on the current pixel row
  form one layer, with unioned sample coverage. A colour change ends the layer; a later occurrence of
  the same colour starts another layer. Table order is part of the specification.
- **Blending:** start with the field and blend layers in that order, per channel:
  `out = (src * c + under * (16 - c) + 8) / 16`, integer division. Text is drawn afterwards.
- **The gradient's colour on a badge pixel row** `row` (0 at the badge's top pixel) uses
  `d = 64k - 1` and `t = clamp(row, 0, d)`. Per channel the colour is
  `(top_c * (d - t) + bottom_c * t + floor(d / 2)) / d`, integer division. This rounds to nearest and
  includes both endpoint colours at rows 0 and `64k - 1`.
- Text is drawn from the font's glyphs as the console draws them.
- **Pixel formats:** RGB or BGR order, 3 or 4 bytes per pixel; the fourth byte is written 255 (opaque).
- Pixels outside the framebuffer are clipped. The renderer writes the composition rectangle, including
  its field pixels, and leaves pixels outside it and row padding untouched. The caller fills the rest
  of the screen with the field colour.

### 6.4 The oracle

`expected_at(px, py, state)` returns the exact colour of a pixel of the logo block from this section alone: it
evaluates the 16 sample points of each layer by the shape inequalities, with no code shared with the
rasterizer's scanline filler, and blends as above. `state` is the crate's `Frame`: completed/total stage
counts, six milestone states and the cursor's blink phase. The oracle takes local composition pixel
coordinates and scale `k`; V2b maps stage masks to this frame. The geometry table and the colour formulas
are the specification both sides share; the rasterization code is not. Because coverage is defined exactly,
the oracle covers antialiased edges and the wordmark as well as interiors.

---

## 7. The splash and its hand-over

The splash is tty1 in `KD_GRAPHICS` owned by the kernel. It is drawn as soon as the target has a framebuffer:
at `fbcon::init_early` on x86-64 (written straight to the framebuffer until the render thread owns it), when
`arm64_fb` initializes the GOP path on Parallels and VMware, and when `virtio-gpu-device` is up on ARM64
QEMU. Stages are recorded lock-free from the first instruction, so stages reached before the screen exists
show as reached when it is first drawn.

**It gives way to tty1's text on the first of these triggers**, once; later triggers have no effect:

| Reason | Trigger |
|---|---|
| `reader` | A process reads from tty1: getty reading a login name, or bsh reading a command |
| `kd-text` | `KDSETMODE(KD_TEXT)` on tty1 (section 4.1's permission rule) |
| `key` | A key press on the keyboard; that key is consumed and not delivered |
| `display-taken` | A process takes the display (suite, probe and program panels, the desktop), or `KDSETMODE(KD_GRAPHICS)` on tty1; the process gets the screen |
| `panic` | A panic or fatal fault; the diagnostics screen takes the screen |
| `backstop` | 10 seconds after PID 1 starts |

- **Init issues `KDSETMODE(KD_TEXT)` on tty1 in `default` mode only**, immediately before it prints
  `[init] Boot script completed`. In `shell` and `console` modes it does not; the `reader` trigger hands
  over.
- A testing kernel (`tests` mode) keeps the splash until the backstop or the end of the run.

**Markers** (V2b lands the exact strings with its code and the stage catalog, replacing the stub in
`docs/boot-path.json`):

| Marker | Printed |
|---|---|
| `[splash] logo drawn: ` | Once, when first drawn, with the resolution, `S`, `k` and the badge's rectangle |
| `[splash] progress shown: ` | Once the bar and dots have been drawn, with the stage count |
| `[splash] logo read back: ` | The first read-back check, when it matched (section 8) |
| `[splash] progress read back: ` | A second read-back check that matched, made at a later stage count and the other blink phase, with both stage counts and bar widths; not printed when the counts or widths are equal |
| `[splash] logo read back differs: ` | Either check failed: fatal |
| `[splash] console took the screen: <reason>` | At the hand-over, in thread context |

**The `boot-logo` boot-path milestone** requires the four kernel stages (drawn, progress shown, read back,
progress read back) on both architectures. Its `shell` list requires those four and
`[splash] console took the screen: reader`, and its `default` list those four and
`[splash] console took the screen: kd-text`. A mode list replaces the architecture list, so each override
repeats the kernel stages, and neither override accepts another reason: a default boot that hands over only at
the backstop does not pass.

---

## 8. Read-back checks and switch logging

- **The read-back** compares the displayed pixels with the shadow, which must be byte-identical, and a set of
  probe pixels with the oracle: `expected_at` for the splash (a lattice of each 8th pixel over the logo
  block's bounding box, plus each pixel of one row through the B and one through the wordmark), and the font
  oracle for console cells. It reports the probe count, the mismatches (which must be 0) and the shadow's
  SHA-256. The render thread runs it; no part of it runs in interrupt context.
- **What is read** (V1): VRAM through the mapping on x86-64, GOP and VMware. On ARM64 QEMU and Parallels VirGL
  boots, the guest backing of the resource bound to the scanout, after a completed transfer and flush of each
  row checked; the check names that resource as the device reported it at the last `SET_SCANOUT` and the flush
  it read after, and fails when the backing read is not the bound resource's or the flush had not completed.
  Marker lines on those boots say `backing after flush`.
- **A switch logs** `[console] foreground ttyN (<source>)` (`key alt-f2`, `key ctrl-alt-f2`, `ioctl`, or
  `chvt`), then the check's verdict: `[console] ttyN shown: read back identical; digest <first 16 hex digits of
  the shadow's SHA-256>` or `[console] ttyN shown: read back differs: <first mismatch>` (fatal). **A switch
  does not log screen contents:** terminals can show secrets, and serial and boot logs are retained.
- **`/proc/console/check`** (root only, mode 0400) is how a case or a metal script reads the screen. Reading it
  runs the check on the foreground VT and returns:

  ```text
  console foreground: tty3
  console check: shadow and screen identical
  console oracle: cells=<n> mismatches=0
  console row: <text>
  ```

  with one `console row:` line per non-blank row of the foreground VT, top to bottom, trailing blanks
  removed, and `console check: shadow and screen differ at x=<x> y=<y>` when they differ.
- **`/proc/fbcon/check`** and its `fbcon check:` and `fbcon row:` lines stay on x86-64 until Metal 4's consumers
  move to `/proc/console/check`, in the same pull request that moves them (V1).

---

## 9. Keys

### 9.1 Bindings

Key codes are the published `input-event-codes.h` values the input core uses (V5). Either Alt key and either
Ctrl key counts; Shift and Super do not change a binding. Only a press (value 1) acts: repeats (value 2) of a
binding's key have no effect, and the release of a consumed press is consumed too.

| Chord | `keymaps(5)` action | On a text VT | On a graphics VT, no grab | Under a compositor grab |
|---|---|---|---|---|
| **Ctrl+Alt+F1 .. F6** | `Console_1` .. `Console_6` | Switch | Switch (section 4.4; refused while a `VT_AUTO` display owner holds) | **Reserved:** reaches the console and switches; the grab holder sees neither the press nor the release |
| **Ctrl+Alt+F7 .. F12** | (no action) | Consumed, no effect | Consumed, no effect | Reserved: consumed, no effect |
| **Alt+F1 .. F6** | `Console_1` .. `Console_6` | Switch | Delivered to the VT as input (`ESC` then the F-key's bytes) | Delivered to the grab holder |
| **Alt+F7 .. F12** | (no action) | Consumed, no effect | Delivered to the VT as input | Delivered to the grab holder |
| **Alt+Left / Alt+Right** | `Decr_Console` / `Incr_Console` | Previous / next VT, wrapping within 1-6 | Delivered to the VT as input | Delivered to the grab holder |
| **Shift+PageUp / PageDown** | `Scroll_Backward` / `Scroll_Forward` | Scroll the view (1.6) | Delivered to the VT as input | Delivered to the grab holder |

- A text VT is one in `KD_TEXT`. A switch key that acts is consumed: no VT sees it as bytes.
- Ctrl+Alt+F1..F12 are reserved so that applications keep Alt+Fn, and so that raising `VT_COUNT` later takes
  no shortcut from an application.
- The foreground VT owns the keyboard: its line discipline gets the bytes, Ctrl+C signals its foreground
  process group, and Caps Lock state is kept per VT.

### 9.2 Grabs

The compositor's grab (window-manager `input-core`, GUI plan section 5.4) takes key events from the console
consumer, except the reserved chords above. **A grab is effective only while the grab holder's VT is the
foreground:** when its VT is released, the input core sends the grab holder releases for each key it holds
and delivers it no further events until its VT is acquired again, when the grab resumes with pressed state reset;
meanwhile key events go to the console consumer for the foreground VT. V5 builds the routing point in the
input core's first slice; the grab itself, and the case that measures the reserved chords under it
(`window-manager/input-core/grab`), belong to the window-manager effort.

---

## 10. Host-injected keys (`HOSTKEY`)

A case that needs a real key from outside the guest asks for it on serial, the launcher injects it, and the
case reads its effect back in the guest. The host key is the input; the in-guest read-back is the evidence.

- **The request** is a suite record, written like VALUE and WAIT (one write starting with a newline, through
  a `libbreenix::suite` helper, between the suite's START and DONE):

  ```text
  SUITE console HOSTKEY <category>/<case> seq=<n> keys=<chord>[,<chord>...]
  ```

  `libbreenix::suite::hostkeys(chords)` emits the record and returns its sequence;
  `hostkey_method()` exposes the declaration. The helper rejects an absent method
  or a chord containing a declared missing key. Sequence ranges are reserved in
  the parent before each case forks, and `--hostkeys` is retained across a suite sequence.

  `seq` increases across the run, so a launcher that sees a line twice (a serial replay) injects it once. A
  chord is modifiers and one key joined by `-`: modifiers `ctrl`, `alt`, `shift`, `super` (left keys) and
  `rctrl`, `ralt` (right keys); keys `a`-`z`, `0`-`9`, `f1`-`f12`, `left`, `right`, `up`, `down`, `pageup`,
  `pagedown`, `home`, `end`, `insert`, `delete`, `enter`, `tab`, `backspace`, `esc`, `space`, `capslock`.
  Each chord is pressed in order (modifiers first, then the key) and released in reverse; chords are sent in
  order.
- **The launcher** watches the serial for HOSTKEY records of its run and injects each within one second: QMP
  `send-key` on ARM64 and x86-64 QEMU, `prlctl send-key-event` on Parallels, and the result of the VMware VNC
  spike on VMware (Q5, recorded here by V6). It logs each injection in its own records, not on the guest's
  serial.
- **The launcher declares its path before the suite starts**, so a case knows what to expect: QEMU passes
  `-fw_cfg name=opt/breenix/hostkeys,string=<method>[;missing=<key>,...]`, the disk launchers write the same
  value as a `hostkeys` field of `/etc/breenix/boot-target`, and the kernel passes it to the suite as
  `--hostkeys <value>` beside `--case`. `missing=` names keys the path cannot send (Parallels refuses extended
  scancodes, which include the arrows and Page Up and Page Down). V6 owns the declaration, the runner's argv,
  the kernel's forwarding and `scripts/suite-verdict.py`'s acceptance of HOSTKEY records.
- **A case fails, and does not skip,** with `no host key path` when no method was declared, with
  `no host key path for <chord> on <method>` when a key it needs is missing, and with
  `host key seq=<n> not received within <ms> ms (<method>)` when the effect does not arrive in time. A case
  waits on its effect (usually `VT_WAITACTIVE` in a child bounded by `case_ms_left()`), then reads back.

### V6 backend findings

Parallels 27.0.3 accepts the basic `prlctl send-key-event` codes. The earlier
probe reported letters and Alt+Fn reaching the guest, but those boots predate
V6's final HOSTKEY record format and do not validate the final watcher.
Extended codes remain declared in `missing=`; a case requiring one fails.
The VNC fallback was tried on a disposable VM configuration: `prlctl set`
with `--vnc-mode manual --vnc-address 127.0.0.1 --vnc-port 5997` returned
`Unrecognized option: --vnc-mode`. The configuration was deleted without booting.
This does not establish a Parallels VNC key path; #1791 tracks the missing
extended-key route without changing target expectations.

**VMware spike (Q5):** the earlier Fusion probe reported an RFB handshake,
framebuffer updates and accepted key events after enabling the built-in VNC
server, but no observed guest key effect. The guest reported
`XHCI command IRQ unavailable`. This is transport evidence only, predates the
final HOSTKEY format, and is not a successful VMware keyboard self-test.
V7 must repair the guest keyboard and rerun the host-key self-test during the
catch-up before VMware key coverage can pass. Without the opt-in VNC port,
the launcher declares no path and cases fail `no host key path`.
The VNC server is unauthenticated; its exposure must be resolved before using
it outside an isolated test environment.

---

## 11. Userspace

- **`chvt N`** (V4u): opens `/dev/tty0`, calls `VT_ACTIVATE(N)` then `VT_WAITACTIVE(N)`, and exits 0. It exits
  1 with `chvt: <error>` on stderr when a call fails or `N` is not a number.
- **`getty <tty>`** (V4u): `getty tty3` opens `/dev/tty3`, calls `setsid` and `TIOCSCTTY`, and runs C4's
  prompt and checks. Without an argument it uses `/dev/console`, as today.
- **`init console`** starts a getty on each VT, tty1 through `/dev/console`, each respawned independently with
  C4's rate limit. **`init shell`** keeps bsh on tty1 and starts no process on tty2 to tty6, so `VT_OPENQRY`
  answers 2. **`init` (default mode)** issues `KDSETMODE(KD_TEXT)` on tty1 as section 7 says.
- The `getty` cases log in with a per-run account the suite writes into `/etc/shadow` on its own disposable
  disk copy (random password, C4's SHA-512-crypt), and remove it afterwards. No image ships a known password.

---

## 12. The suite

`docs/suites/console.json` lists the cases; `/sbin/suite-console` runs them. Each category is one effort
milestone's measure, built by the lane named here:

| Category | Milestone | Lane |
|---|---|---|
| `screen` | `screen` | V1 |
| `emulation` | `emulation` | V3 |
| `vt-devices`, `vt-switch`, `scrollback` | `virtual-terminals` | V4 (the scrollback view keys with V5) |
| `chvt`, `getty` | `chvt-getty` | V4u |
| `keys` | `keyboard` | V5, with V6's host keys and, on Parallels and VMware, V7's USB keyboard |

`keys/alt-left-right-wrap` and `keys/shift-pageup` remain scored on Parallels and
fail while its extended-key host path is absent (#1791). Resolving that path or
obtaining an explicit target-scope ruling is required; V6 does not reduce these
expectations to make this target green.

- **Until its layer exists, a case fails naming it:** `missing <layer> (<lane>): <what the kernel answered>`.
  The suite asks the kernel each time: `/proc/console/check` for the screen layers, and `VT_GETSTATE` on
  `/dev/console` for the terminal layers. Once the kernel answers, a case whose check has not been written yet
  fails saying so. No case skips. A lane writes a case's check from this contract in its own commit, before
  the implementation it measures.
- **The suite takes the display only after its last case** (Q17), so its cases run on the console itself. That
  is the `Suite::defer_display()` runner option, supplied by D0 and enabled by the console skeleton. The runner
  neither takes nor maps the display and draws no panel during cases; it opens the display for the final panel
  after the last case. Other suites keep their live panels.
- The suite is `timingSensitive`: its cases wait on switches and on host keys.
