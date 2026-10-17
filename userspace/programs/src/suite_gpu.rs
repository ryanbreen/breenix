//! GPU rendering: the `gpu` effort's suite, measuring the contract in `docs/gui/gpu.md`.
//!
//! One category per effort milestone, in build order: `device`, `submit`, `readback`,
//! `textures`, `profile`, `pipelines`, `isolation`, `present` and `performance`. Every case
//! runs on every GPU-era target (ARM64 QEMU on the GL QEMU, Parallels and VMware).
//!
//! The suite is written before the layers it measures. Each case names the layer it needs
//! and the build step that supplies it (`docs/gui/gpu.md` section 1.11), and until that layer
//! exists the case fails saying so: `missing <layer> (G<n>): <what the kernel answered>`. It
//! asks the kernel every time, through syscall 416's VERSION op, so the reason is measured
//! rather than assumed. When the kernel answers VERSION, a case whose check has not been
//! written yet still fails, and says that. No case skips. A lane writes a case's check in its
//! own commit, from the contract, before the implementation it measures.
//!
//! `device/version` was the first real check; the `device` and `submit` categories' checks
//! (G1) are in `suite_gpu/device.rs` and `suite_gpu/submit.rs`, `readback`'s (G2) in
//! `suite_gpu/readback.rs`, and `textures`' with `pipelines/copy` (G3) in
//! `suite_gpu/textures.rs`, `profile`'s (G4) in `suite_gpu/profile.rs`, `pipelines`' (G5) in
//! `suite_gpu/pipelines.rs` and `isolation`'s (G6) in `suite_gpu/isolation.rs`.

extern crate alloc;
#[path = "suite_gpu/expectations.rs"]
pub mod expectations;
pub use expectations::{error, float, outline, work};

use libbreenix::suite::{case, category, check, fail, suite, CaseResult, Suite};
use libbreenix::syscall::raw;

#[path = "suite_gpu/common.rs"]
mod common;
#[path = "suite_gpu/device.rs"]
mod device;
#[path = "suite_gpu/submit.rs"]
mod submit;
#[path = "suite_gpu/readback.rs"]
mod readback;
#[path = "suite_gpu/textures.rs"]
mod textures;
#[path = "suite_gpu/profile.rs"]
mod profile;
#[path = "suite_gpu/pipelines.rs"]
mod pipelines;
#[path = "suite_gpu/isolation.rs"]
mod isolation;

static SUITE: Suite = suite("gpu", "GPU rendering", &[
    category("device", "3D device & capability profile", &[
        case("version", "Syscall 416's VERSION op returns 1", device_version),
        case("feature-negotiated", "The device's 3D feature is negotiated: VirGL, or SVGA3D DX contexts", device::feature_negotiated),
        case("caps-parsed", "CAPS returns the profile record from the device's capability set, each capability id once", device::caps_parsed),
        case("profile-formats", "B8G8R8A8 and R8G8B8A8 render and sample, and R8 samples, by the device's own capability entries", device::profile_formats),
        case("profile-integer-ops", "The device reports integer shader operations, integer texel fetch and region copies", device::profile_integer_ops),
        case("profile-limits", "CAPS reports a maximum target size of at least 2560x1600 and at most 16384", device::profile_limits),
        case("profile-record", "The CAPS record has its documented version and size, with reserved fields zero", device::profile_record),
        case("missing-capability-fails", "A masked profile capability fails initialization naming it", device::missing_capability_fails),
        case("no-fallback", "With a capability masked every op refuses with that reason and nothing renders another way", device::no_fallback),
        case("display-info", "The device reports its scanouts and their sizes", device::display_info),
        case("backend-named", "CAPS names backend 4 (VirGL) or 5 (SVGA3D DX), the device found on the bus", device::backend_named),
        case("device-error-reported", "A forced device error reaches the submitting context as EIO naming the device's answer", device::device_error_reported),
    ]),
    category("submit", "command submission & fences", &[
        case("context-create", "CONTEXT_CREATE returns an owned context within 10 s that draws while its fixed rendering catalogue warms, reports the warm-up complete, and its first PROBE warms probe_int", submit::context_create),
        case("context-destroy", "CONTEXT_DESTROY frees the context once its submitted work has completed", submit::context_destroy),
        case("context-limit", "Creating a context past the per-process quota returns EAGAIN", submit::context_limit),
        case("stream-validate", "A malformed command stream returns EINVAL with the failing command's index", submit::stream_validate),
        case("validation-atomic", "A stream that fails validation sends nothing: its targets keep their generation and bytes", submit::validation_atomic),
        case("stream-overflow", "A stream past the draw, instance or buffer limits is refused whole, never truncated", submit::stream_overflow),
        case("fence-order", "One context's fences complete in submission order", submit::fence_order),
        case("fence-wait", "WAIT returns 0 once its fence completes, and the target it wrote is published", submit::fence_wait),
        case("wait-timeout", "WAIT on an incomplete fence returns ETIMEDOUT at its timeout", submit::wait_timeout),
        case("wait-eintr", "A signal interrupts WAIT with EINTR", submit::wait_eintr),
        case("wait-killable", "A process killed in WAIT exits within 250 ms", submit::wait_killable),
        case("fence-foreign", "WAIT on another context's fence is refused", submit::fence_foreign),
        case("multi-context", "Two contexts of one process submit independently and both read back exactly", submit::multi_context),
        case("ring-backpressure", "A fifth submission while four are in flight waits for room, and none is lost", submit::ring_backpressure),
        case("device-error-eio", "A submission the device answers with an error completes its fence with EIO", submit::device_error_eio),
        case("recovery-after-error", "After a device error the next valid frame renders and reads back exactly", submit::recovery_after_error),
        case("frame-one-syscall", "A frame of any number of draws is one SUBMIT call", submit::frame_one_syscall),
        case("freed-resource-refused", "A stream naming a destroyed resource returns EINVAL", submit::freed_resource_refused),
        case("fence-echo-checked", "A fence answered for another submission is not taken as completion", submit::fence_echo_checked),
        case("counters", "COUNTERS reports submissions, draws, instances, area, work estimate, uploads and readbacks exactly", submit::counters_exact),
        case("watchdog-quarantine", "A fence withheld for 2 s quarantines its context, whose fences fail with EIO device-hung", submit::watchdog_quarantine),
        case("device-reset-recovers", "A device with no progress is reset: every context gets EIO device-reset and new work renders exactly", submit::device_reset_recovers),
    ]),
    category("readback", "device readback", &[
        case("full-target", "A full-target readback after a clear equals the clear colour in every byte", readback::full_target),
        case("poison-unwritten", "Each readback is two transfers into complementary poisons that must agree, so no byte is unwritten", readback::poison_unwritten),
        case("scenes-aba", "Scenes A, B, A read back as A, B, A exactly", readback::scenes_aba),
        case("partial-rect", "Partial rectangles, including rows that are not a multiple of 256 bytes, read back exactly", readback::partial_rect),
        case("padded-stride", "Caller strides wider than the row, including ones not a multiple of 256 bytes, read back exactly", readback::padded_stride),
        case("format-canonical", "Both target formats read back as canonical BGRA bytes", readback::format_canonical),
        case("record-fields", "Record v2 is 1024 bytes with every field set as the contract defines and reserved bytes zero", readback::record_fields),
        case("device-path", "The record names the backend's readback sequence and its answers", readback::device_path),
        case("fence-identity", "The record's render and readback fences are the ones the device answered for them", readback::fence_identity),
        case("ordered-after-draws", "A readback after a submission sees every draw of it", readback::ordered_after_draws),
        case("fault-skip-transfer", "A skipped transfer-back is refused naming it, and reads back exactly once disarmed", readback::fault_skip_transfer),
        case("fault-stale-fence", "A transfer fence answered for another transfer is refused naming it", readback::fault_stale_fence),
        case("fault-wrong-resource", "A readback of another resource is refused naming it", readback::fault_wrong_resource),
        case("fault-corrupt-pixel", "A pixel corrupted outside the draw fails the exact comparison at that pixel", readback::fault_corrupt_pixel),
        case("fault-suppress-draw", "A draw dropped from the device stream while its fence completes fails the exact comparison", readback::fault_suppress_draw),
        case("error-invalidates-targets", "A failed submission's targets read back EIO until a later submission writes them whole", readback::error_invalidates_targets),
        case("owner-only", "Only the display owner reads back the scanout", readback::owner_only),
        case("efault", "A readback into unmapped or read-only memory returns EFAULT with no side effect", readback::efault),
        case("large-target", "A 2560x1600 target reads back exactly", readback::large_target),
    ]),
    category("textures", "textures & uploads", &[
        case("create-formats", "Textures are created in B8G8R8A8, R8G8B8A8 and R8, and other formats are refused", textures::create_formats),
        case("create-limits", "Sizes from 1x1 to the device maximum are created, and zero or larger sizes are refused", textures::create_limits),
        case("upload-full", "A full upload reads back exactly", textures::upload_full),
        case("upload-subrect", "A sub-rectangle upload changes only that rectangle", textures::upload_subrect),
        case("upload-clip", "An upload past the texture edge is clipped by pixel, never past a row's end", textures::upload_clip),
        case("upload-stride", "Uploads with caller strides, including ones not a multiple of 256 bytes, read back exactly", textures::upload_stride),
        case("upload-3byte", "3-byte pixel uploads expand to 4 bytes with alpha 255", textures::upload_3byte),
        case("upload-visibility", "An upload is visible to the next submission, both read back and sampled", textures::upload_visibility),
        case("unaligned-pitch", "R8 64 wide and BGRA 48 wide, rows not a multiple of 256 bytes, round-trip exactly", textures::unaligned_pitch),
        case("copy-region", "A GPU region copy moves exactly the named rectangle", textures::copy_region),
        case("copy-overlap", "A copy between overlapping rectangles of one target gives the source as it was before the copy", textures::copy_overlap),
        case("immutable-while-pinned", "An upload to a texture an in-flight submission samples does not change what it samples", textures::immutable_while_pinned),
        case("destroy-deferred", "A destroyed texture still held by a submission is freed only after its fence completes", textures::destroy_deferred),
        case("quota", "Textures past the GPU memory quota return ENOMEM, and freeing them restores the counter", textures::quota),
        case("r8-atlas", "An R8 atlas, including one whose rows are not a multiple of 256 bytes, samples exactly", textures::r8_atlas),
        case("readback-uploaded", "An uploaded texture reads back unchanged", textures::readback_uploaded),
        case("efault-upload", "An upload from unmapped memory returns EFAULT with no side effect", textures::efault_upload),
        case("fault-wrong-texture", "A draw bound to another of the process's textures fails the exact comparison", textures::fault_wrong_texture),
        case("fault-skip-upload", "An upload whose transfer to the host is omitted is caught by reading the texture back", textures::fault_skip_upload),
    ]),
    category("profile", "deterministic rendering profile", &[
        case("clear-exact", "A clear writes its four bytes to every pixel of both formats", profile::clear_exact),
        case("unorm-roundtrip", "All 256 values of each channel round-trip through the shader's UNORM conversion", profile::unorm_roundtrip),
        case("texel-fetch-exact", "Integer texel fetch returns each texel's bytes from all three formats", profile::texel_fetch_exact),
        case("integer-ops", "The 32-bit integer routines return their literal results, range ends included", profile::integer_ops),
        case("wide-arithmetic", "Two- and three-word products and 96-step division return their literal results", profile::wide_arithmetic),
        case("binary32-emulation", "The emulated binary32 routines return their literal results bit for bit", profile::binary32_emulation),
        case("int-float-exact", "Integers below 2^24 pass through float vertex positions exactly", profile::int_float_exact),
        case("fragcoord-integer", "The floor of each fragment's position is its pixel's integer coordinates", profile::fragcoord_integer),
        case("rect-coverage", "A two-triangle rectangle writes every pixel inside it and none outside", profile::rect_coverage),
        case("rect-edges", "Rectangles one pixel wide or tall, and at the target's edges, write exactly their pixels", profile::rect_edges),
        case("scissor-exact", "A scissor clips to exactly its half-open rectangle", profile::scissor_exact),
        case("viewport-exact", "Integer corners map to exactly their pixels with a top-left origin", profile::viewport_exact),
        case("alpha-write", "Alpha is written as given, 0 and partial values included, with blending off", profile::alpha_write),
        case("swizzle-bgra-rgba", "Each channel lands in its own byte in both target formats", profile::swizzle_bgra_rgba),
        case("no-dither", "A uniform colour writes identical pixels everywhere: no dithering", profile::no_dither),
        case("no-srgb", "Values are written without sRGB encoding or decoding", profile::no_srgb),
        case("determinism", "The same stream rendered twice reads back identical bytes", profile::determinism),
        case("cross-context", "The same stream in two contexts reads back identical bytes", profile::cross_context),
        case("large-coordinates", "Each shape predicate routine returns its literal results at coordinates near 2^31", profile::large_coordinates),
        case("filtered-sampler-refused", "A filtered sampler in an exact pipeline returns EINVAL", profile::filtered_sampler_refused),
        case("fixed-blend-off", "Fixed-function blending is off in every exact pipeline", profile::fixed_blend_off),
        case("profile-violation-detected", "A planted filtered-sampling draw is refused naming the violation", profile::profile_violation_detected),
    ]),
    category("pipelines", "drawing pipelines", &[
        case("solid-rects", "solid fills half-open integer rectangles in one colour", pipelines::solid_rects),
        case("rect-batch", "Rectangles batched in one draw equal the same rectangles drawn one by one", pipelines::rect_batch),
        case("copy", "copy places a texture rectangle at a target point, converting format by channel", textures::pipeline_copy),
        case("copy-clip", "copy clips to the target and to the clip rectangle", pipelines::copy_clip),
        case("copy-scaled", "copy_scaled replicates texels by an integer factor exactly", pipelines::copy_scaled),
        case("over-premultiplied", "over matches the contract's source-over for all 65536 colour and alpha pairs on four backgrounds", pipelines::over_premultiplied),
        case("over-straight", "over_straight premultiplies once, then matches over", pipelines::over_straight),
        case("over-overlap", "Overlapping blended primitives in one draw give the sequential source-over result", pipelines::over_overlap),
        case("coverage-blend", "coverage blends an R8 mask with a colour by the contract's coverage rule", pipelines::coverage_blend),
        case("coverage-lcd", "coverage's LCD variant blends each channel by its own coverage", pipelines::coverage_lcd),
        case("coverage-spans-square", "coverage_spans matches the contract's literal table for the square", pipelines::coverage_spans_square),
        case("coverage-spans-winding", "coverage_spans matches the reference rasterizer bit for bit on the winding cases", pipelines::coverage_spans_winding),
        case("coverage-spans-corpus", "coverage_spans matches the reference rasterizer on every printable glyph at the five reference sizes", pipelines::coverage_spans_corpus),
        case("bitmap-text", "bitmap_text draws the 5x7 and 16 px fonts exactly at integer scales", pipelines::bitmap_text),
        case("line-bresenham", "line matches the contract's literal line tables", pipelines::line_bresenham),
        case("line-clip", "A clipped line draws exactly the in-target pixels of the unclipped walk", pipelines::line_clip),
        case("line-extreme", "Lines with endpoints near 2^31 draw exactly their in-target pixels", pipelines::line_extreme),
        case("circle-fill", "circle_fill draws exactly the contract's filled-circle pixel set", pipelines::circle_fill),
        case("circle-outline", "circle_outline draws exactly the contract's outline pixel set", pipelines::circle_outline),
        case("rounded-rect", "rounded_rect draws exactly the contract's rounded-rectangle pixel set", pipelines::rounded_rect),
        case("gradient", "gradient_v matches the contract's integer interpolation at every height", pipelines::gradient),
        case("triangle-integer", "triangle draws integer-vertex triangles by edge functions with the top-left rule", pipelines::triangle_integer),
        case("box-filter", "box_filter downscales by the contract's bin means, rounded half up", pipelines::box_filter),
        case("nearest-scale", "nearest_scaled maps each pixel by the contract's floor(xW/w)", pipelines::nearest_scale),
        case("clip-nested", "Nested clips intersect, and popping one restores the clip before it", pipelines::clip_nested),
        case("order", "Overlapping unblended primitives land in submission order", pipelines::order),
    ]),
    category("isolation", "per-process GPU isolation", &[
        case("handle-private", "Handles are per process: another process's handle numbers name nothing here", isolation::handle_private),
        case("handle-guess", "Guessed handle values are refused with EINVAL", isolation::handle_guess),
        case("fork-no-handles", "A forked child inherits no GPU handles", isolation::fork_no_handles),
        case("exec-reclaims", "exec releases every context and resource of the process", isolation::exec_reclaims),
        case("exit-reclaims", "exit releases every context and resource, and the GPU memory counter returns to its baseline", isolation::exit_reclaims),
        case("kill-mid-submit", "Killing a process mid-submit leaves the device and other contexts working", isolation::kill_mid_submit),
        case("killed-work-drains", "A killed process's queued work drains, and its resources are freed when their fences complete", isolation::killed_work_drains),
        case("quota-memory", "GPU memory past the per-process quota returns ENOMEM", isolation::quota_memory),
        case("quota-handles", "Handles past the per-process quota are refused", isolation::quota_handles),
        case("quota-contexts", "One process at its context quota does not limit another's", isolation::quota_contexts),
        case("work-estimate-refused", "A stream one fragment-step over the work cap returns EDQUOT", isolation::work_estimate_refused),
        case("foreign-target-draw", "Drawing to another process's target returns EINVAL from the validator, and nothing reaches the device", isolation::foreign_target_draw),
        case("foreign-readback", "Reading back another process's target is refused", isolation::foreign_readback),
        case("display-context-unreachable", "No client stream can name the display owner's context or targets", isolation::display_context_unreachable),
        case("no-addresses", "No op returns a kernel or device address", isolation::no_addresses),
        case("fuzz-streams", "10000 seeded malformed streams each return EINVAL and their index, and the next frame reads back exactly", isolation::fuzz_streams),
        case("bounded-validation", "Validation time is bounded by the stream's length", isolation::bounded_validation),
        case("efault-submit", "A stream at an unmapped address returns EFAULT with no side effect", isolation::efault_submit),
        case("efault-readonly", "A readback record into read-only memory returns EFAULT with no side effect", isolation::efault_readonly),
        case("quiet-errors", "No GPU error path prints kernel output", isolation::quiet_errors),
        case("unknown-op", "An unknown op returns EINVAL", isolation::unknown_op),
        case("test-hooks-absent", "Test-control points 19 to 31 are refused to every process but the latched PID 1", isolation::test_hooks_absent),
        case("killable-wait", "SIGKILL ends a process blocked in CONTEXT_CREATE warm-up, WAIT, READBACK or PRESENT within 250 ms", isolation::killable_wait),
        case("client-cannot-scanout", "A process that is not the display owner can neither present nor set a mode", isolation::client_cannot_scanout),
        case("irq-masked-bound", "The longest IRQ-masked interval in the GPU paths stays within 500 us under a full-screen workload", isolation::irq_masked_bound),
    ]),
    category("present", "display modes, HiDPI & present", &[
        case("modes-listed", "MODES lists 1280x800 at scale 1 and 2560x1600 at scale 2, with the device's maximum", present_modes_listed),
        case("mode-1280x800", "At 1280x800 a full-screen target presents and reads back exactly", present_mode_1280x800),
        case("mode-2560x1600", "At 2560x1600 a full-screen target presents and reads back exactly", present_mode_2560x1600),
        case("scale-reported", "The scale factor is published with the mode", present_scale_reported),
        case("scanout-bind", "PRESENT binds the drawn target to the scanout, and the record names it", present_scanout_bind),
        case("flip", "Two targets alternate on the scanout, each reading back exactly while presented", present_flip),
        case("present-completion", "PRESENT's fence completes when the device has answered", present_present_completion),
        case("present-owner-only", "PRESENT and SET_MODE are refused to every process but the display owner", present_present_owner_only),
        case("readback-presented", "The presented target read back on the scanout equals its drawing", present_readback_presented),
        case("takeover-giveback", "Taking the display and giving it back presents the previous owner's target exactly", present_takeover_giveback),
        case("ownership-stack", "Four nested display owners restore in order, and a fifth is refused", present_ownership_stack),
        case("console-restored", "Giving the display back to the kernel redraws its console on the GPU", present_console_restored),
        case("firmware-handoff", "The firmware framebuffer is written only until the GPU driver presents its first frame", present_firmware_handoff),
        case("mode-change-content", "After a mode change the scanout reads back exactly as drawn, with no stale pixels", present_mode_change_content),
        case("fault-skip-flush", "A skipped present flush is detected and refused naming it", present_fault_skip_flush),
        case("fault-wrong-scanout", "A present of the wrong target is detected by the record", present_fault_wrong_scanout),
    ]),
    category("performance", "frame rate & latency", &[
        case("clear-rate-2560", "At least 60 full-screen 2560x1600 clears per second, each presented", performance_clear_rate_2560),
        case("rect-throughput", "10000 solid 16x16 rectangles in one submission complete within 50 ms", performance_rect_throughput),
        case("submit-latency", "An empty submission's fence: median at most 2 ms and maximum at most 10 ms over 1000", performance_submit_latency),
        case("first-draw-latency", "A new context's first draw with each fixed rendering-catalogue pipeline (excluding probe_int) completes within 10 ms", performance_first_draw_latency),
        case("readback-time-2560", "A full 2560x1600 readback completes within 100 ms", performance_readback_time_2560),
        case("upload-bandwidth", "A 16 MiB upload is visible to the next submission within 100 ms", performance_upload_bandwidth),
        case("coverage-throughput", "A full-screen 2560x1600 coverage blend from an R8 mask completes within 16.7 ms", performance_coverage_throughput),
        case("wait-no-spin", "CPU time while waiting on a fence is at most 5% of the wait", performance_wait_no_spin),
        case("present-rate-2560", "At least 60 presents per second alternating two full-screen 2560x1600 targets", performance_present_rate_2560),
    ]),
]);

// The layers cases need, each with the build step that supplies it.
const CORE: &str = "the kernel GPU core and syscall 416 (G1)";
const MODES: &str = "display modes and scale (G7)";
const PRESENT: &str = "two-target present (G7)";
const OWNER: &str = "the GPU display ownership stack (G7)";
const CONSOLE: &str = "the kernel console on the GPU (G7)";
const PRESENT_CHECKS: &str = "the present checks (G7)";
const PERF: &str = "the 2560x1600 performance work (G8)";

/// Syscall 416, `gpu(op, arg, len)` (`docs/gui/gpu.md` section 1.4).
const GPU: u64 = 416;
/// Op 0, VERSION: takes no argument and returns the interface version.
const OP_VERSION: u64 = 0;
/// The interface version this suite measures.
const GPU_VERSION: i64 = 1;
const ENOSYS: i64 = 38;

/// Syscall 416's VERSION op, as the raw return: the version, or a negative errno.
fn gpu_version() -> i64 {
    // SAFETY: VERSION takes no argument, so no pointer is passed and len is 0.
    unsafe { raw::syscall3(GPU, OP_VERSION, 0, 0) as i64 }
}

/// What the kernel answered VERSION, as a reason.
fn version_answer(ret: i64) -> String {
    match ret {
        r if r == -ENOSYS => String::from("syscall 416 VERSION returned ENOSYS"),
        r if r < 0 => format!("syscall 416 VERSION returned errno {}", -r),
        r => format!("syscall 416 VERSION returned {r}, expected {GPU_VERSION}"),
    }
}

/// Fail a case whose layer is not there yet, naming the layer and what the kernel answered.
fn missing(need: &str) -> CaseResult {
    match gpu_version() {
        GPU_VERSION => fail(format!("missing {need}: this case's check is not written yet")),
        ret => fail(format!("missing {need}: {}", version_answer(ret))),
    }
}

/// One function per case that has no check yet, each failing with the layer it needs.
macro_rules! needs {
    ($($name:ident => $need:expr;)*) => {
        $(fn $name() -> CaseResult { missing($need) })*
    };
}

fn device_version() -> CaseResult {
    let ret = gpu_version();
    check(ret == GPU_VERSION, &format!("missing {CORE}: {}", version_answer(ret)))?;
    let _case = common::begin()?;
    Ok(())
}

needs! {
    present_modes_listed => MODES;
    present_mode_1280x800 => MODES;
    present_mode_2560x1600 => MODES;
    present_scale_reported => MODES;
    present_scanout_bind => PRESENT_CHECKS;
    present_flip => PRESENT;
    present_present_completion => PRESENT;
    present_present_owner_only => PRESENT;
    present_readback_presented => PRESENT;
    present_takeover_giveback => OWNER;
    present_ownership_stack => OWNER;
    present_console_restored => CONSOLE;
    present_firmware_handoff => CONSOLE;
    present_mode_change_content => MODES;
    present_fault_skip_flush => "test-control point 25 skip-present-flush (G7)";
    present_fault_wrong_scanout => "test-control point 26 wrong-scanout (G7)";
    performance_clear_rate_2560 => PERF;
    performance_rect_throughput => PERF;
    performance_submit_latency => PERF;
    performance_first_draw_latency => PERF;
    performance_readback_time_2560 => PERF;
    performance_upload_bandwidth => PERF;
    performance_coverage_throughput => PERF;
    performance_wait_no_spin => PERF;
    performance_present_rate_2560 => PERF;
}

fn main() {
    // `isolation/exec-reclaims` re-executes this binary to look at the process after exec.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(isolation::EXEC_PROBE) {
        isolation::exec_probe(&args[2..]);
    }
    SUITE.run()
}
