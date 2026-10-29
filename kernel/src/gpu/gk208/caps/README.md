# Executable admission buffers

These are Breenix-authored literal inputs and device-output expectations for
M8.2-u6, not captured hardware pixels or external goldens. No shader asset is
changed. The eight capabilities use 80 by 16 pixels. BGRA/RGBA surfaces span
five GOB columns and two GOB rows; R8 spans two columns and two rows. Thus a
one-GOB echo cannot establish a format or block-height result.

For row-major pixel index n = 80*y+x, colour.bgra stores literal bytes
(n & 255, n >> 8, (73*n+19) & 255, 128+n%128). The first two bytes identify
1280 of 1280 pixels uniquely. colour.rgba exchanges byte 0 and byte 2. Solid draws
consume the packed BGRA colour as their input and write either target format;
sampling both targets produces colour.bgra. Targets are first CE-filled with
0x3d so a suppressed draw cannot inherit the preceding probe's expected image.

An R8 texel has only 256 possible values, so a single R8 image cannot uniquely
identify 1280 pixels. r8.inputs contains two 1280-byte planes: n&255, then
37*(n>>8)+11. Their pairs uniquely identify the 1280 positions, and each plane
crosses GOB boundaries. r8.bgra concatenates the corresponding output images,
with each texel mapped to literal BGRA (0,0,r,255). Both planes must complete
and both whole images must match under both readback poisons.

integer.inputs stores 1280 little-endian (a,b,l,r) tuples:
a = 0x81b70401 XOR (0x9e3779b9*n), b = 0xfedcba98 XOR (0x7f4a7c15*n),
l = n&31, r = (n>>5)&31. integer.bgra contains the literal little-endian q
words specified by tools/kepler/README.md's u3 integer-ops formula, evaluated
with wrapping u32 arithmetic. Its 1280 output words are distinct. The runner
uses the committed u3 program with one tuple per pixel through u4's quad
builder. The texel-fetch program draws the whole colour image through u5's
TIC/TSC path using integer offset (0,0).

region.bgra starts as colour.bgra and replaces destination (2,1,73,13) with
source (3,2,73,13) from the original image. The source and destination are the
same resource, exercising u2's two-launch overlap staging. The full result,
including the unchanged surrounding pixels, must match.

Each operation completes on the startup owner's native device before two
separate CE readbacks into 0x55 and 0xaa. Admission compares both full buffers
to these literal bytes in addition to requiring completion.
Eight caps-probe JSON rows retain both images and expected hashes in the bulk
network evidence stream. Serial carries row digests and elapsed time only.
The startup session is restored before selecting GK208 builders and probing;
NATIVE_STARTING serializes startup and CORE_STARTED excludes old boot probes.
Installation follows the probes. Failed hardware work retains its owner and
backing; mismatched bytes leave the named capability unprobed.
