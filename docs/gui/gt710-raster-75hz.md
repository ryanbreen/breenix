G25 follow-up (2026-10-31): the coordinator authorized one new 75 Hz
console/GPU diagnostic with underflow status cleared and armed after the
modeset has completed and settled. The prior measurement window began
before the modeset; its sticky status therefore did not distinguish a
transition from steady scanout. The diagnostic keeps the 60 Hz fallback
when DMT 24h is absent. A fresh post-arm underflow still fails, including
one racing the pause before the next modeset. Acceptance of this retry is
pending the card captures; G25 requires returning to the preferred raster
if they show a fresh underflow.

# Advertised 75 Hz raster (24.7-raster-75hz)

**Historical:** the earlier 75 Hz capture reported sticky underflow status; G23 was withdrawn and G24 restored 108 MHz. G25 now tests whether that bit came from the transition, with a post-settle measurement window.

The earlier unit's claim of direct operator authorization was incorrect;
the coordinator corrected that attribution on 2026-10-31. G25 is the current
authority for this diagnostic. The implementation uses the existing native
backend and shared GPU core. The sections below retain the earlier unit's
source and host-validation record; they do not establish card acceptance.

## Selection and source decisions

The live-DDC bytes retained from `20261031T041513Z-metal` describe LCD17/19.
The test fixture is EDID only; its SHA-256 is recorded in
`tests/fixtures/gt710-edid/README.md`. Byte 36 bit 0 advertises established
1280x1024 at 75 Hz, while the first DTD remains 1280x1024 at 108 MHz.
The chosen DMT 1.13 ID 24h timing is:

| Field | Value |
|---|---|
| Pixel clock | 135000 kHz |
| Horizontal active / sync start / sync end / total | 1280 / 1296 / 1440 / 1688 |
| Vertical active / sync start / sync end / total | 1024 / 1025 / 1028 / 1066 |
| Syncs | Positive horizontal and vertical |

That is approximately 75.0247 Hz and 79.9763 kHz, within this EDID's
49..76 Hz vertical, 25..81 kHz horizontal and 160 MHz clock limits.
The independent host/scorer literals use this DMT timing. The source
cross-check is Linux v6.8 `drm_edid.c`, whose own header contains the MIT
grant: `drm_dmt_modes` ID 0x24 and `edid_est_modes`/`add_established_modes`.
Its complete original notice is in the two adapted consumers and its hash
and consumer mapping are in `THIRD_PARTY_NOTICES`. This is a file-specific
license check; it does not admit Linux DRM integration code.

The parser adds this established timing to the advertised inventory.
`native_raster` selects that exact tuple only when its size matches the
first DTD. Otherwise it returns the first DTD. The inventory keeps the DTD
first, preserving the existing board-evidence contract and allocation
geometry. This unit adds the requested established bit; it does not expand
the standard-timing catalogue. Takeover and low/high/low switching use the
same selector. The existing board PLL limits, fractional calculation,
board scripts, supervisor and active-state checks program the selected
clock; there is no fixed board-specific PLL coefficient in the driver.

The G20 logical modes remain 1280x800 and 2560x1600. Their viewport methods
are compared across the two rasters by the host check. G21 console extent,
latched-surface receipts and firmware-write checks retain their properties;
the scorer selects the G25 diagnostic raster for native timing, PLL and middle
switch expectations. The full native pixel comparison still runs, and a
coherent 108 MHz timing/register/PLL record is rejected when 75 Hz is
advertised. Without the advertisement the 108 MHz record passes.

At 75.0247 Hz, 119 latch intervals take approximately 1.5861 s; allowing a
full extra interval gives approximately 1.5995 s for 120 presents. This is
a raster budget calculation, not measured native throughput. CPU submission,
GPU execution and observation still count against the unchanged 60/s suite
bounds. Present completion continues to require the real latch and present
fence in scanout readback.

## Check first and validation

Check commit `75404c63` preceded the implementation. The first warning-free
Rust run was 21 PASS / 1 FAIL because the 75 Hz timing was absent. The two
new scorer checks failed at the missing EDID timing/inventory. An initial
Rust harness compilation reported unused viewport APIs under `-Dwarnings`;
the check was completed to execute those APIs, without suppressing warnings.
The committed check includes the resulting viewport execution.

The exact host PLL limits are stated in the check: 27 MHz reference,
500..1000 MHz VCO, 1..27 MHz input and the enumerated divider bounds. For
135 MHz it requires M=2, N=69, P=7, fraction=4096 (coefficient 0x074502);
for the 108 MHz fallback it requires M=2, N=71, P=9, fraction=4096.
These are exact results for the test limits, not captured VBIOS limits or
claimed hardware PLL readbacks. The production calculation is unchanged
MIT nouveau `pllgt215.c`, with its original notice already in `modeset.rs`.

The final modeset check passes 22 Rust cases, the EDID check passes two,
and the new native raster scorer check passes two. Both Rust compilations
use `-Dwarnings`; commands run with `CARGO_BUILD_JOBS=4` under `taskpolicy -b`.
Five G21 receipt checks and six runtime display evidence checks also pass.
The affected board/native/switch/flip report run passed ten checks and
found two failures because the presentation title had changed in the scorer
without its matching manifest entry. The manifest title was synchronized;
only those two checks were rerun after that repair, and both pass.
A test-discovery command accidentally selected unrelated report modules;
it was interrupted during an aperture report check, then replaced by an
explicit list of affected display checks. That interrupted run is not a
validation result.

The x86 production release build (`cargo build --release --bin qemu-uefi`)
passes in 1m33s without warnings. The changed kernel files are native GK208
files; shared GPU core and ARM implementations are unchanged, so there is
no ARM build in this unit. The kernel embeds the existing committed Kepler
assets under G5; no shader/firmware/toolchain regeneration is needed.

No boot, persistent session, management-controller access, PR or merge is
part of this unit. Full 170/170 hardware acceptance, the console 330 s
handoff-to-DONE bound and the GPU boot's own elapsed-time report remain for
the acceptance unit. No native performance result is claimed here.

Claim-lint notes (the extensionless scorer and notices are reviewed directly;
the linter skips them):

```
claim-lint: scripts/claim-lint.py --files tests/gt710_modeset_test.rs tests/gt710_raster_report_test.py tests/fixtures/gt710-edid/README.md -> exit 1 (comment reworded), then exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-raster-75hz/check-message -> exit 0
claim-lint: scripts/claim-lint.py --files kernel/src/gpu/gk208/edid.rs kernel/src/gpu/gk208/modeset.rs kernel/src/gpu/gk208/native.rs tests/gt710_edid_test.rs docs/gui/gt710-raster-75hz.md -> exit 0
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-raster-75hz.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-raster-75hz/change-message -> exit 0
```
