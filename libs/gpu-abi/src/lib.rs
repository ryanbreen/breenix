//! Syscall 416's binary interface, shared by the kernel and userspace.
//!
//! `gpu(op, arg, len)` and its rules are `docs/gui/gpu.md` section 1.4; the argument
//! layouts, the BGL command stream and the codes in this crate are `docs/gui/gpu-abi.md`.
//! Every structure is little-endian and `#[repr(C)]` with its offsets fixed; reserved
//! fields are zero on the way in and the kernel writes them zero on the way out.
//!
//! [`stream`] defines the command stream and the validator the kernel runs over a
//! submission before anything reaches the device. [`float`] builds IEEE-754 binary32 bit
//! patterns from integers with integer arithmetic, for a kernel that never touches the FPU.

#![no_std]

pub mod float;
pub mod stream;

/// `test_control(op, arg, len)` (`docs/gui/gpu.md` section 1.8, BreenGL contract section 21).
pub const SYS_TEST_CONTROL: u64 = 415;
/// `gpu(op, arg, len)`.
pub const SYS_GPU: u64 = 416;
/// What `VERSION` returns.
pub const INTERFACE_VERSION: i64 = 1;
/// The pipeline catalogue's version (`docs/gui/gpu.md` section 1.3).
pub const CATALOGUE_VERSION: u32 = 1;

/// Op numbers of syscall 416.
pub mod op {
    pub const VERSION: u32 = 0;
    pub const CAPS: u32 = 1;
    pub const CONTEXT_CREATE: u32 = 2;
    pub const CONTEXT_DESTROY: u32 = 3;
    pub const RESOURCE_CREATE: u32 = 4;
    pub const RESOURCE_DESTROY: u32 = 5;
    pub const UPLOAD: u32 = 6;
    pub const SUBMIT: u32 = 7;
    pub const WAIT: u32 = 8;
    pub const READBACK: u32 = 9;
    pub const MODES: u32 = 10;
    pub const SET_MODE: u32 = 11;
    pub const PRESENT: u32 = 12;
    pub const COUNTERS: u32 = 13;
    /// The highest op the table defines; anything above is EINVAL.
    pub const LAST: u32 = 13;
}

/// The errno values the interface returns (Linux numbering), negated in the return value.
pub mod errno {
    pub const EPERM: i64 = 1;
    pub const EINTR: i64 = 4;
    pub const EIO: i64 = 5;
    pub const EAGAIN: i64 = 11;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const ENODEV: i64 = 19;
    pub const EINVAL: i64 = 22;
    pub const ENOSYS: i64 = 38;
    pub const ETIMEDOUT: i64 = 110;
    pub const EDQUOT: i64 = 122;
}

/// A type every bit pattern of which is valid, so it can be copied to and from user
/// memory and zero-initialised.
///
/// # Safety
/// The type is `#[repr(C)]`, has no padding the layout leaves undefined, and holds only
/// integers and arrays of integers.
pub unsafe trait Plain: Copy {
    /// The all-zero value.
    const ZERO: Self;
}

/// Backends `CAPS` and record v2 name.
pub mod backend {
    pub const NONE: u32 = 0;
    pub const VIRGL: u32 = 4;
    pub const SVGA3D_DX: u32 = 5;
}

/// `Caps::status`: whether the device was admitted to the profile.
pub mod status {
    /// The device meets the profile; every op is available.
    pub const ADMITTED: u32 = 0;
    /// No GPU device with a 3D path was found.
    pub const NO_DEVICE: u32 = 1;
    /// The device lacks the capability `Caps::missing` names.
    pub const MISSING_CAPABILITY: u32 = 2;
    /// The device was found but failed to initialise.
    pub const DEVICE_FAILED: u32 = 3;
}

/// The profile's capabilities (`docs/gui/gpu.md` section 1.5, item 8), as `Caps::missing`
/// and test-control point 23's parameter name them.
pub mod capability {
    pub const NONE: u32 = 0;
    /// The device's 3D feature: VirGL negotiated, or SVGA3D DX contexts.
    pub const FEATURE_3D: u32 = 1;
    /// A capability set the backend can read the profile from.
    pub const CAPABILITY_SET: u32 = 2;
    pub const BGRA_RENDER: u32 = 3;
    pub const BGRA_SAMPLE: u32 = 4;
    pub const RGBA_RENDER: u32 = 5;
    pub const RGBA_SAMPLE: u32 = 6;
    pub const R8_SAMPLE: u32 = 7;
    /// Integer shader operations.
    pub const INTEGER_OPS: u32 = 8;
    /// Integer-addressed texel fetch.
    pub const TEXEL_FETCH: u32 = 9;
    /// GPU region copies.
    pub const REGION_COPY: u32 = 10;
    /// A maximum 2D target size of at least 2560x1600.
    pub const TARGET_SIZE: u32 = 11;
    pub const LAST: u32 = 11;

    /// The name the kernel's refusal reason uses.
    pub const fn name(id: u32) -> &'static str {
        match id {
            FEATURE_3D => "3d-feature",
            CAPABILITY_SET => "capability-set",
            BGRA_RENDER => "b8g8r8a8-render",
            BGRA_SAMPLE => "b8g8r8a8-sample",
            RGBA_RENDER => "r8g8b8a8-render",
            RGBA_SAMPLE => "r8g8b8a8-sample",
            R8_SAMPLE => "r8-sample",
            INTEGER_OPS => "integer-ops",
            TEXEL_FETCH => "texel-fetch",
            REGION_COPY => "region-copy",
            TARGET_SIZE => "target-size",
            _ => "none",
        }
    }
}

/// `Caps::features`.
pub mod feature {
    /// VirGL: `VIRTIO_GPU_F_VIRGL` negotiated.
    pub const VIRGL: u32 = 1 << 0;
    /// VirGL: `VIRTIO_F_VERSION_1` negotiated.
    pub const VERSION_1: u32 = 1 << 1;
    /// SVGA3D: the DX-context capability entry is set.
    pub const DX_CONTEXT: u32 = 1 << 8;
}

/// `Caps::profile`.
pub mod profile {
    pub const INTEGER_OPS: u32 = 1 << 0;
    pub const TEXEL_FETCH: u32 = 1 << 1;
    pub const REGION_COPY: u32 = 1 << 2;
}

/// BGL pixel formats (also record v2's `format`).
pub mod format {
    pub const B8G8R8A8: u32 = 0;
    pub const R8G8B8A8: u32 = 1;
    pub const R8: u32 = 2;

    /// Bytes per pixel.
    pub const fn bytes(format: u32) -> u32 {
        match format {
            B8G8R8A8 | R8G8B8A8 => 4,
            R8 => 1,
            _ => 0,
        }
    }
}

/// `FormatCaps::flags`.
pub mod format_flag {
    pub const SAMPLE: u32 = 1 << 0;
    pub const RENDER: u32 = 1 << 1;
}

/// The pipeline catalogue (`docs/gui/gpu.md` section 1.3), numbered in the table's order.
pub mod pipeline {
    pub const SOLID: u32 = 0;
    pub const COPY: u32 = 1;
    pub const COPY_SCALED: u32 = 2;
    pub const NEAREST_SCALED: u32 = 3;
    pub const BOX_FILTER: u32 = 4;
    pub const OVER: u32 = 5;
    pub const OVER_STRAIGHT: u32 = 6;
    pub const COVERAGE: u32 = 7;
    pub const COVERAGE_SPANS: u32 = 8;
    pub const BITMAP_TEXT: u32 = 9;
    pub const LINE: u32 = 10;
    pub const CIRCLE_FILL: u32 = 11;
    pub const CIRCLE_OUTLINE: u32 = 12;
    pub const ROUNDED_RECT: u32 = 13;
    pub const GRADIENT_V: u32 = 14;
    pub const TRIANGLE: u32 = 15;
    pub const PROBE_INT: u32 = 16;
    pub const COUNT: u32 = 17;

    /// Per-fragment steps of a pipeline whose cost does not depend on its geometry
    /// (section 1.4.1, the work estimate). `None` for pipelines not built yet.
    pub const fn steps(pipeline: u32) -> Option<u64> {
        match pipeline {
            SOLID => Some(1),
            _ => None,
        }
    }
}

/// `RESOURCE_CREATE` kinds.
pub mod kind {
    pub const TARGET: u32 = 1;
    pub const TEXTURE: u32 = 2;
    pub const BUFFER: u32 = 3;
}

/// A fence's outcome, as `Wait::status` reports it.
pub mod fence_status {
    /// The device completed the submission.
    pub const OK: u32 = 0;
    /// The device answered the submission with an error; `Wait::answer` is its answer.
    /// Two in a row on one context quarantine that context only.
    pub const DEVICE_ERROR: u32 = 1;
    /// The watchdog found the submission hung and quarantined its context.
    pub const DEVICE_HUNG: u32 = 2;
    /// The device was reset with the submission outstanding (a device fault, or the
    /// watchdog's no-progress condition).
    pub const DEVICE_RESET: u32 = 3;
    /// The device answered the submission with another submission's fence.
    pub const FENCE_MISMATCH: u32 = 4;
}

/// A context's state, as `Counters::state` reports it.
pub mod context_state {
    pub const LIVE: u64 = 0;
    /// The watchdog, or two device errors in a row, quarantined it: further submissions
    /// are refused with EIO.
    pub const QUARANTINED: u64 = 1;
    /// The device was reset under it: every op on it is refused with EIO.
    pub const RESET: u64 = 2;
}

/// Limits of section 1.4.1 and the kernel's quotas.
pub mod limits {
    /// Draws per submission.
    pub const DRAWS: u64 = 65_536;
    /// Instances per submission.
    pub const INSTANCES: u64 = 1_048_576;
    /// Bytes of buffers one submission may name.
    pub const BUFFER_BYTES: u64 = 64 << 20;
    /// Estimated fragment-steps per submission.
    pub const WORK: u64 = 1 << 31;
    /// Submissions in flight per context.
    pub const IN_FLIGHT: usize = 4;
    /// The longest stream `SUBMIT` reads, in bytes.
    pub const STREAM_BYTES: u64 = 80 << 20;
    /// Contexts per process.
    pub const CONTEXTS: usize = 8;
    /// The largest target or texture side.
    pub const SIDE: u32 = 16_384;
    /// The watchdog's hang threshold and its no-progress threshold before a reset.
    pub const WATCHDOG_NS: u64 = 2_000_000_000;
}

/// One format's capability entry in [`Caps`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatCaps {
    pub format: u32,
    pub flags: u32,
}

/// One scanout in [`Caps`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanoutInfo {
    pub enabled: u32,
    pub width: u32,
    pub height: u32,
    pub reserved: u32,
}

/// `CAPS`: the profile record (512 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Caps {
    /// 1.
    pub version: u32,
    /// 512.
    pub size: u32,
    /// One of [`status`].
    pub status: u32,
    /// The [`capability`] that kept the device out, when `status` says so.
    pub missing: u32,
    /// One of [`backend`].
    pub backend: u32,
    /// [`feature`] bits.
    pub features: u32,
    /// The capability set the profile was read from, and its version.
    pub capset_id: u32,
    pub capset_version: u32,
    /// How many distinct capability set ids the device lists, and which (ascending).
    pub capset_count: u32,
    pub capset_ids: [u32; 8],
    /// B8G8R8A8, R8G8B8A8 and R8, in that order.
    pub formats: [FormatCaps; 3],
    /// [`profile`] bits.
    pub profile: u32,
    /// The device's shading-language level from its capability set.
    pub shading_level: u32,
    /// The largest target: the device's 2D maximum, capped at 16,384.
    pub max_width: u32,
    pub max_height: u32,
    pub catalogue_version: u32,
    /// Bit `n` set: catalogue pipeline `n` is implemented.
    pub pipelines: u32,
    /// How many scanouts the device reports.
    pub scanout_count: u32,
    pub reserved0: [u32; 2],
    /// The device's renderer string from its capability set, NUL-padded.
    pub renderer: [u8; 64],
    /// Why the device is not admitted, NUL-padded; empty when it is.
    pub reason: [u8; 64],
    pub scanouts: [ScanoutInfo; 16],
}

/// `CONTEXT_CREATE` (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextCreate {
    /// Out: the new context's handle.
    pub handle: u64,
    /// In: 0.
    pub flags: u32,
    /// Out: how many catalogue programs creation compiled and warmed.
    pub warmed: u32,
    pub reserved: [u64; 2],
}

/// `CONTEXT_DESTROY` and `RESOURCE_DESTROY` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub handle: u64,
    pub reserved: u64,
}

/// `RESOURCE_CREATE` (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceCreate {
    /// One of [`kind`].
    pub kind: u32,
    /// One of [`format`]; 0 for a buffer.
    pub format: u32,
    /// Pixels, for a target or texture; 0 for a buffer.
    pub width: u32,
    pub height: u32,
    /// Bytes, for a buffer; 0 otherwise.
    pub size: u64,
    /// Out: the new resource's handle.
    pub handle: u64,
    pub reserved: [u64; 2],
}

/// `SUBMIT` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submit {
    pub context: u64,
    /// The stream's address and length in bytes (a multiple of 4).
    pub stream: u64,
    pub length: u64,
    /// Out: the submission's fence.
    pub fence: u64,
    /// Out, on EINVAL or EDQUOT: the failing command's index and [`stream::error`] code.
    pub error_index: u32,
    pub error: u32,
    /// Out: the submission's work estimate in fragment-steps.
    pub estimate: u64,
    pub reserved: [u64; 2],
}

/// `WAIT` (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wait {
    pub context: u64,
    pub fence: u64,
    /// Nanoseconds; 0 polls.
    pub timeout_ns: u64,
    /// Out: the fence's [`fence_status`] once it has completed.
    pub status: u32,
    /// Out: the device's answer for a `DEVICE_ERROR` (its response type).
    pub answer: u32,
    pub reserved: [u64; 2],
}

/// `READBACK` (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readback {
    pub target: u64,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Where the canonical BGRA rows go, how many bytes are there, and the row stride.
    pub pixels: u64,
    pub pixels_len: u64,
    pub stride: u32,
    /// Out on EIO: one of `readback_reason`; zero on input.
    pub reason: u32,
    /// Where the 1,024-byte record v2 goes.
    pub record: u64,
    /// Out on target-invalid: the submission that invalidated it; zero on input.
    pub failed_submission: u64,
}

/// `READBACK`'s EIO reasons (`docs/gui/gpu-abi.md`, the `READBACK` table). Every EIO
/// names exactly one; none is reported under another's name.
pub mod readback_reason {
    pub const NONE: u32 = 0;
    /// The two complementary-poison observations differ.
    pub const UNWRITTEN: u32 = 1;
    /// A transfer's answer lacks the fence flag or echoes another fence.
    pub const FENCE_MISMATCH: u32 = 2;
    /// An observation claiming the scanout names a resource other than the confirmed binding.
    pub const RESOURCE_MISMATCH: u32 = 3;
    /// The target is invalid; `failed_submission` names the submission that made it so.
    pub const TARGET_INVALID: u32 = 4;
    /// The device answered a readback transfer with an error.
    pub const DEVICE_ERROR: u32 = 5;
    /// The watchdog ended a transfer the device had not answered.
    pub const DEVICE_HUNG: u32 = 6;
    /// A device reset was decided while a transfer was outstanding.
    pub const DEVICE_RESET: u32 = 7;
    /// The target was lost to an earlier device reset.
    pub const TARGET_LOST: u32 = 8;
    /// The target's handle was destroyed while the readback waited for its writers.
    pub const TARGET_DROPPED: u32 = 9;
    /// A `PRESENT` binding or replacing the target was sent and has not completed.
    pub const BINDING_UNKNOWN: u32 = 10;
}

/// `PRESENT` (32 bytes). Binds the whole target to scanout 0 and flushes it in
/// `context`, one of the caller's live contexts.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Present {
    pub target: u64,
    /// Out: a fence of `context` (`ns << 32 | n`, in sequence with its `SUBMIT`s);
    /// zero on input.
    pub fence: u64,
    /// Out: the device's answer to the flush; zero on input.
    pub answer: u32,
    pub reserved: u32,
    /// In: the context the bind and flush are sent in.
    pub context: u64,
}

/// `COUNTERS` (256 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counters {
    /// In: a context handle, or 0 for the whole calling process.
    pub context: u64,
    pub submissions: u64,
    pub draws: u64,
    pub instances: u64,
    pub fragment_area: u64,
    pub work_estimate: u64,
    pub uploads_image: u64,
    pub uploads_fixture: u64,
    pub readbacks: u64,
    /// Submissions that waited for room because four were in flight.
    pub ring_waits: u64,
    /// Fences the device answered with an error.
    pub device_errors: u64,
    /// A context's [`context_state`]; 0 for the process.
    pub state: u64,
    /// The process's live context handles.
    pub contexts: u64,
    /// The process's device contexts not yet destroyed, including destroyed handles
    /// whose work has not completed.
    pub device_contexts: u64,
    /// The process's live resource handles.
    pub resources: u64,
    /// Guest backing bytes the process's resources hold.
    pub resource_bytes: u64,
    /// For a context: how many catalogue programs its creation warmed.
    pub warmed: u64,
    pub reserved: [u64; 15],
}

/// Record v2 (`docs/gui/gpu.md` section 1.8), as raw bytes with its field offsets.
pub mod record {
    pub const SIZE: usize = 1024;
    pub const VERSION: u32 = 2;
    pub const VERSION_OFF: usize = 0;
    pub const SIZE_OFF: usize = 4;
    pub const SERIAL: usize = 8;
    pub const BACKEND: usize = 16;
    pub const BOUNDARY: usize = 20;
    pub const FLAGS: usize = 24;
    pub const FORMAT: usize = 28;
    pub const X: usize = 32;
    pub const Y: usize = 36;
    pub const WIDTH: usize = 40;
    pub const HEIGHT: usize = 44;
    pub const TARGET_WIDTH: usize = 48;
    pub const TARGET_HEIGHT: usize = 52;
    pub const STRIDE: usize = 56;
    pub const CANONICAL: usize = 60;
    pub const CONTEXT: usize = 64;
    pub const TARGET: usize = 72;
    pub const RESOURCE: usize = 80;
    pub const GENERATION: usize = 88;
    pub const RENDER_SERIAL: usize = 96;
    pub const RENDER_FENCE: usize = 104;
    pub const RENDER_STATUS: usize = 112;
    pub const RENDER_KIND: usize = 116;
    pub const READBACK_FENCE: usize = 120;
    pub const READBACK_STATUS: usize = 128;
    pub const READBACK_KIND: usize = 132;
    pub const READBACK_SURFACE: usize = 136;
    pub const READBACK_MOB: usize = 144;
    pub const COPY_KIND: usize = 152;
    pub const COPY_STATUS: usize = 156;
    pub const POISON: usize = 160;
    pub const UNWRITTEN: usize = 164;
    pub const COMMIT: usize = 168;
    pub const COMPOSITE_SERIAL: usize = 176;
    pub const PRESENT_FENCE: usize = 184;
    pub const PRESENT_STATUS: usize = 192;
    pub const WINDOW_COUNT: usize = 196;
    pub const WINDOWS: usize = 200;
    pub const RENDERER: usize = 712;
    pub const RESERVED: usize = 776;
    /// Boundary code 4: the host renderer and window are outside the guest's view.
    pub const BOUNDARY_HOST: u32 = 4;
}

/// Test-control points the effort owns (`docs/gui/gpu.md` section 1.8), with the ops of
/// system call 415. Every point is one-shot: it affects the next occurrence of its event,
/// counts one firing and disarms itself.
pub mod test_point {
    pub const FIRST: u32 = 19;
    pub const SKIP_TRANSFER: u32 = 19;
    pub const STALE_FENCE: u32 = 20;
    pub const WRONG_RESOURCE: u32 = 21;
    pub const CORRUPT_PIXEL: u32 = 22;
    pub const MASK_CAPABILITY: u32 = 23;
    pub const PROFILE_VIOLATION: u32 = 24;
    pub const SKIP_PRESENT_FLUSH: u32 = 25;
    pub const WRONG_SCANOUT: u32 = 26;
    pub const DEVICE_ERROR: u32 = 27;
    pub const SUPPRESS_DRAW: u32 = 28;
    pub const WRONG_TEXTURE: u32 = 29;
    pub const SKIP_UPLOAD_VISIBILITY: u32 = 30;
    pub const WITHHOLD_FENCE: u32 = 31;
    pub const LAST: u32 = 31;

    /// `WITHHOLD_FENCE`'s parameter: ignore one fence's completion (the default); stop
    /// taking any completion from the device until it is reset; or fence depth: keep the
    /// answers to the next context's first four submissions unpublished until a fifth
    /// `SUBMIT` on that context waits at the four-in-flight limit.
    pub const WITHHOLD_ONE: u64 = 0;
    pub const WITHHOLD_ALL: u64 = 1;
    pub const WITHHOLD_DEPTH: u64 = 2;

    /// Whether the effort has built the point. The rest of 19 to 31 are reserved: they
    /// arm (behind the same latch and caller checks) and do nothing until their layer
    /// is built.
    pub const fn built(point: u32) -> bool {
        matches!(point, SKIP_TRANSFER | STALE_FENCE | WRONG_RESOURCE | CORRUPT_PIXEL | MASK_CAPABILITY | DEVICE_ERROR | SUPPRESS_DRAW | WITHHOLD_FENCE)
    }
}

/// System call 415's ops and arguments (BreenGL contract section 21).
pub mod test_control {
    pub const STATE: u32 = 0;
    pub const ARM: u32 = 1;
    pub const DISARM_ALL: u32 = 2;
    pub const FIRED: u32 = 3;
}

/// `test_control` `ARM` (24 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcArm {
    pub point: u32,
    pub reserved: u32,
    pub target: u64,
    pub param: u64,
}

/// `test_control` `FIRED` (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcFired {
    pub point: u32,
    pub reserved: u32,
    pub count: u64,
}

macro_rules! plain {
    ($($t:ty),*) => {
        $(
            // SAFETY: each is #[repr(C)] over integers and integer arrays, with its
            // fields laid out without implicit padding (checked by the sizes below).
            unsafe impl Plain for $t {
                const ZERO: Self = unsafe { core::mem::zeroed() };
            }
        )*
    };
}

plain!(Caps, ContextCreate, Handle, ResourceCreate, Submit, Wait, Readback, Present, Counters, TcArm, TcFired);

const _: () = {
    assert!(core::mem::size_of::<FormatCaps>() == 8);
    assert!(core::mem::size_of::<ScanoutInfo>() == 16);
    assert!(core::mem::size_of::<Caps>() == 512);
    assert!(core::mem::offset_of!(Caps, formats) == 68);
    assert!(core::mem::offset_of!(Caps, profile) == 92);
    assert!(core::mem::offset_of!(Caps, renderer) == 128);
    assert!(core::mem::offset_of!(Caps, reason) == 192);
    assert!(core::mem::offset_of!(Caps, scanouts) == 256);
    assert!(core::mem::size_of::<ContextCreate>() == 32);
    assert!(core::mem::size_of::<Handle>() == 16);
    assert!(core::mem::size_of::<ResourceCreate>() == 48);
    assert!(core::mem::size_of::<Submit>() == 64);
    assert!(core::mem::size_of::<Wait>() == 48);
    assert!(core::mem::size_of::<Readback>() == 64);
    assert!(core::mem::offset_of!(Readback, reason) == 44);
    assert!(core::mem::offset_of!(Readback, failed_submission) == 56);
    assert!(core::mem::size_of::<Present>() == 32);
    assert!(core::mem::offset_of!(Present, fence) == 8);
    assert!(core::mem::offset_of!(Present, answer) == 16);
    assert!(core::mem::offset_of!(Present, context) == 24);
    assert!(core::mem::size_of::<Counters>() == 256);
    assert!(core::mem::size_of::<TcArm>() == 24);
    assert!(core::mem::size_of::<TcFired>() == 16);
};
