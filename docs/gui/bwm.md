# bwm contract

E18 rulings W3-W5, W9 and W10 amend the GUI plan. This document specifies
behaviour for the implementation lanes; it is not a report of acceptance.
The window interface is [BreenGL sections 17-20](breengel.md#17-the-window-interface-version-2).

## Coordinates and chrome

Rectangles are half-open physical pixel rectangles. Let S be display scale,
(X,Y) the frame origin, and (W,H) the owner's LIST content size. Client sizes
are already physical pixels. Only chrome dimensions are multiplied by S.

| Element | Scale 1 | Scale 2 |
|---|---|---|
| Top bar height | 28 | 56 |
| Bottom appbar height | 36 | 72 |
| Border | 2 | 4 |
| Titlebar | 32 | 64 |
| Content origin relative to frame | (2,34) | (4,68) |
| Frame extent | (W+4,H+36) | (W+8,H+72) |
| Shadow offset right/down | 3 | 6 |
| Painted extent including shadow | (W+7,H+39) | (W+14,H+78) |
| Close/minimize button extent | (20,20) | (40,40) |
| Close origin from frame's right/top | (-26,7) | (-52,14) |
| Minimize origin relative to close | (-24,0) | (-48,0) |
| Resize grab zone | 8 | 16 |
| Minimum outer window | (120,116) | (240,232) |
| Cascade offset | (50,50) | (100,100) |
| Minimum visible titlebar strip on a side | 32 | 64 |

The cascade uses (0,28S,display_width,display_height-64S). For each axis,
room=max(0,usable_extent-frame_extent), margin=min(30S,room) horizontally
and min(10S,room) vertically; position=usable_origin+margin+
(index*50S mod (room-margin+1)). Thus a fitting first frame starts at
(30,38) at scale 1 or (60,76) at scale 2. The title is UTF-8 up to 64 bytes,
clipped at the chrome's available text area. Titles, configured font metrics,
button positions and appbar order are independent test expectations.
The appbar lists normal windows in creation order, regardless of stacking;
button width is clamp(title's measured advance+16S,60S,180S), with 4S inset,
2S inter-button gap, 4S vertical inset and height 28S.

| Region | RGB |
|---|---|
| Focused / unfocused title | (40,100,220) / (45,50,65) |
| Focused / unfocused border | (60,130,255) / (50,55,70) |
| Focused / unfocused title text | (255,255,255) / (160,165,175) |
| Close button / text | (200,50,50) / (255,255,255) |
| Minimize button / text | (80,85,100) / (255,255,255) |
| Shadow | (8,10,18) |
| Unpublished client content | (12,14,20) |
| Top bar / text | (20,22,30) / (180,185,195) |
| Appbar / top edge | (25,28,38) / (50,55,70) |
| Appbar normal / focused / minimized entry | (40,45,60) / (40,100,220) / (30,33,45) |

At physical row y of a display height D, t=floor(255y/D). The background
RGB is (12+floor(t/12),16+floor(t/20),38+floor(t/6)). On a grid line,
x mod (64S)=0 or y mod (64S)=0, add (6,8,12), saturating each channel.
The shadow is an opaque frame-sized rectangle shifted by (3S,3S), beneath
the frame. The cursor is above the desktop, bars, dimmer and clients; its
shape/footprint are BreenGL section 18's sprite at scale S.

## Chromeless windows (W10)

A title beginning with byte 1 requests chromeless presentation. There is no
border, titlebar, shadow, appbar entry or resize edge. A small chromeless
window starts at (max(0,floor((display_width-W)/2)),floor(display_height/5));
a window covering the mode starts at (0,0). Its content rectangle is (W,H).
It is stacked above normal windows and both bars. Beneath the chromeless
layer, cover the entire physical mode, bars included, with a black dimmer:
output channel floor(channel/2). There is exactly one dimmer while one or
more chromeless windows are visible, and none otherwise. Pointer clicks
inside reach the chromeless client; a click outside dismisses it without
activating an obscured normal client. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

## Policy and owner-mediated resize

Desk state is keyed by stable window id. Focus names an eligible visible
window or none; no minimized window receives input. The registry's geometry,
z order, visibility, minimized and focused flags agree with the desk.
Focus transitions send one lost/gained pair to the actual old/new ids.
Keys release to their press recipient. Pointer gestures hold an implicit
grab through release, with enter/leave and click counts in WindowEvent.
Repeat uses a 500 ms delay and 50 ms period on the UI clock. Double click
and Super double tap use event timestamps with inclusive interval bounds;
the double-click distance bound is 4 physical pixels on either axis. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

Under B41/W5, a resize request changes no frame or owner buffer. bwm keeps
at most one outstanding ResizeRequested per window until LIST changes size,
then frames that reported size, preserving the opposite anchored edge.
A pending request at compositor death leaves the owner's size unchanged.
An owner refusing allocation reports ResizeFailed and retains its old size.
Screen bounds and 4096-pixel buffer limits apply before unsigned conversion.

## Clock, status and completed frames

bwm consumes `/dev/input/events` and uses the UI clock. Syscall 413 is only
the legacy 16-byte pointer snapshot after lane 3 removes bwm's calls (W9).
The compositor status and acknowledgment operations are defined in BreenGL
17.1. An acknowledgment names the claim epoch, UI epoch, handled source
serial and either the successful composite serial or zero (`none-needed`).
It is published after handling and due timers complete. An idle epoch
acknowledges with zero and does not compose. Scene version increases when
geometry, stacking or visibility changes; HIT_TEST names the topmost client
at a physical point and returns that same version atomically. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->

`/proc/breenix/compositor` exposes the status as named decimal fields,
including `epoch=N` for the acknowledged UI epoch. `/proc/breenix/composites`
is the successful device-completion history specified by the suite contract.
Submission acceptance, an FPS print and an acknowledged epoch cannot stand
in for native completed pixels. The four `[bwm]` boot-path markers retain
the bytes catalogued in `docs/boot-path.json`.

`/etc/bwm.conf` accepts `font=<path>`, `clock=frozen` and `fps=off` for the
test configuration. The frozen wall base is 2026-01-01 00:00:00 UTC plus UI
time. Malformed settings fall back to defaults with a visible error. Fonts
and `/etc/hotkeys.conf` load on ARM64 as on other targets.

## Clipboard (W4)

The clipboard is a kernel service on window syscall 414, not a bwm socket.
Its SET, GET and SUBSCRIBE arguments and authority are in BreenGL 17.1.
A caller owns a registered window in the current desktop (ENOENT otherwise, W15).
W22 keeps one clipboard per effective uid within each desktop claim. SET, GET
and SUBSCRIBE reach only the caller's euid, including root, which reaches its
own clipboard. E18 offers no copying between users. Only
`text/plain;charset=utf-8` (mime 1) is supported; another mime is EINVAL.
Text is length-counted UTF-8 including NUL, at most 16 MiB; oversize SET is
E2BIG and preserves the previous text and serial. Empty GET succeeds.

A successful SET replaces the text atomically and assigns a strictly
increasing clipboard change serial. Storage is charged to the setter's
RLIMIT_AS, including while text outlives its registered window or process;
the retained charge is released on replacement or clearing. Window closure
ends ownership but does not delete retained text. A replacement tells the
previous live owner of that euid with ClipboardLost. Subscribed windows of
that euid receive
ClipboardChanged made by the kernel, never forged by WRITE_EVENTS. GET
reports the stored change serial, so concurrent setters agree on the later
successful SET within their euid. A change never wakes or informs another
user's subscribers, nor advances that user's change serial. Releasing or
losing the compositor claim clears every user's text, subscriptions and owner; the next compositor starts empty. No client may
read or set clipboard data using another process's window id. <!-- claim-lint:ok: Normative E18 contract allocation, W1-W13 and #2367; not implementation acceptance. -->
