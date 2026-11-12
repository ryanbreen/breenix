//! Programs used by the compositor and its chrome, including later interactions.

use libbreenix::gpu::abi::pipeline;

pub const SCENE_PIPELINES: u32 = (1 << pipeline::SOLID) | (1 << pipeline::COPY);
// Include bitmap fallback as well as TrueType LCD coverage and later clock/title draws.
pub const CHROME_PIPELINES: u32 = SCENE_PIPELINES | (1 << pipeline::NEAREST_SCALED) | (1 << pipeline::COVERAGE) | (1 << pipeline::COVERAGE_SPANS) | (1 << pipeline::BITMAP_TEXT);
