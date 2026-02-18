//! Graphics-related system calls.
//!
//! Provides syscalls for querying and drawing to the framebuffer.

extern crate alloc;

// Architecture-specific framebuffer imports
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
use crate::logger::SHELL_FRAMEBUFFER;
#[cfg(target_arch = "aarch64")]
use crate::graphics::arm64_fb::SHELL_FRAMEBUFFER;

#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
use crate::graphics::primitives::{Canvas, Color, Rect, fill_rect, draw_rect, fill_circle, draw_circle, draw_line};
use super::SyscallResult;

/// Counter for fb_flush syscalls (diagnostic — read from timer heartbeat)
#[cfg(target_arch = "aarch64")]
pub static FB_FLUSH_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Framebuffer info structure returned by sys_fbinfo.
/// This matches the userspace FbInfo struct in libbreenix.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[repr(C)]
pub struct FbInfo {
    /// Width in pixels
    pub width: u64,
    /// Height in pixels
    pub height: u64,
    /// Stride (pixels per scanline, may be > width for alignment)
    pub stride: u64,
    /// Bytes per pixel (typically 3 or 4)
    pub bytes_per_pixel: u64,
    /// Pixel format: 0 = RGB, 1 = BGR, 2 = U8 (grayscale)
    pub pixel_format: u64,
}

/// Maximum valid userspace address (canonical lower half)
/// Addresses above this are kernel space and must be rejected.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
const USER_SPACE_MAX: u64 = crate::memory::layout::USER_STACK_REGION_END;

/// sys_fbinfo - Get framebuffer information
///
/// # Arguments
/// * `info_ptr` - Pointer to userspace FbInfo structure to fill
///
/// # Returns
/// * 0 on success
/// * -EFAULT if info_ptr is invalid or in kernel space
/// * -ENODEV if no framebuffer is available
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub fn sys_fbinfo(info_ptr: u64) -> SyscallResult {
    // Validate pointer: must be non-null and in userspace address range
    if info_ptr == 0 {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Reject kernel-space pointers to prevent kernel memory corruption
    if info_ptr >= USER_SPACE_MAX {
        log::warn!("sys_fbinfo: rejected kernel-space pointer {:#x}", info_ptr);
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Validate the entire FbInfo struct fits in userspace
    let end_ptr = info_ptr.saturating_add(core::mem::size_of::<FbInfo>() as u64);
    if end_ptr > USER_SPACE_MAX {
        log::warn!("sys_fbinfo: buffer extends into kernel space");
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // On ARM64, use the lock-free FbInfoCache to avoid contention with BWM's
    // fb_flush, which holds SHELL_FRAMEBUFFER for ~400μs during full-screen
    // pixel copies. Framebuffer dimensions are immutable after init.
    #[cfg(target_arch = "aarch64")]
    let info = {
        let cache = match crate::graphics::arm64_fb::FB_INFO_CACHE.get() {
            Some(c) => c,
            None => {
                log::warn!("sys_fbinfo: No framebuffer available");
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
        };
        FbInfo {
            width: cache.width as u64,
            height: cache.height as u64,
            stride: cache.stride as u64,
            bytes_per_pixel: cache.bytes_per_pixel as u64,
            pixel_format: if cache.is_bgr { 1 } else { 0 },
        }
    };

    // On x86_64, acquire the framebuffer lock to read dimensions.
    // Use try_lock with bounded spin since this is a one-time startup call.
    #[cfg(not(target_arch = "aarch64"))]
    let info = {
        let fb = match SHELL_FRAMEBUFFER.get() {
            Some(fb) => fb,
            None => {
                log::warn!("sys_fbinfo: No framebuffer available");
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
        };
        let fb_guard = {
            let mut guard = None;
            for _ in 0..65536 {
                if let Some(g) = fb.try_lock() {
                    guard = Some(g);
                    break;
                }
                core::hint::spin_loop();
            }
            match guard {
                Some(g) => g,
                None => {
                    log::warn!("sys_fbinfo: framebuffer lock busy after 65536 spins");
                    return SyscallResult::Err(super::ErrorCode::Busy as u64);
                }
            }
        };
        use crate::graphics::primitives::Canvas;
        FbInfo {
            width: fb_guard.width() as u64,
            height: fb_guard.height() as u64,
            stride: fb_guard.stride() as u64,
            bytes_per_pixel: fb_guard.bytes_per_pixel() as u64,
            pixel_format: if fb_guard.is_bgr() { 1 } else { 0 },
        }
    };

    // Copy to userspace (pointer already validated above)
    unsafe {
        let info_out = info_ptr as *mut FbInfo;
        core::ptr::write(info_out, info);
    }

    SyscallResult::Ok(0)
}

/// sys_fbinfo - Stub for non-interactive mode (returns ENODEV)
#[cfg(not(any(target_arch = "aarch64", feature = "interactive")))]
pub fn sys_fbinfo(_info_ptr: u64) -> SyscallResult {
    // No framebuffer available in non-interactive mode
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// Draw command operations for sys_fbdraw
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[repr(u32)]
#[allow(dead_code)]
pub enum FbDrawOp {
    /// Clear the left pane with a color
    Clear = 0,
    /// Fill a rectangle: x, y, width, height, color
    FillRect = 1,
    /// Draw rectangle outline: x, y, width, height, color
    DrawRect = 2,
    /// Fill a circle: cx, cy, radius, color
    FillCircle = 3,
    /// Draw circle outline: cx, cy, radius, color
    DrawCircle = 4,
    /// Draw a line: x1, y1, x2, y2, color
    DrawLine = 5,
    /// Flush the framebuffer (for double-buffering)
    Flush = 6,
}

/// Draw command structure passed from userspace.
/// Must match the FbDrawCmd struct in libbreenix.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[repr(C)]
pub struct FbDrawCmd {
    /// Operation code (FbDrawOp)
    pub op: u32,
    /// First parameter (x, cx, x1, or unused)
    pub p1: i32,
    /// Second parameter (y, cy, y1, or unused)
    pub p2: i32,
    /// Third parameter (width, radius, x2, or unused)
    pub p3: i32,
    /// Fourth parameter (height, y2, or unused)
    pub p4: i32,
    /// Color as packed RGB (0x00RRGGBB)
    pub color: u32,
}

/// Get the width of the left (demo) pane
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[allow(dead_code)]
fn left_pane_width() -> usize {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(fb_guard) = fb.try_lock() {
            fb_guard.width() / 2
        } else {
            0
        }
    } else {
        0
    }
}

/// Get the height of the framebuffer
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[allow(dead_code)]
fn fb_height() -> usize {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        if let Some(fb_guard) = fb.try_lock() {
            fb_guard.height()
        } else {
            0
        }
    } else {
        0
    }
}

/// sys_fbdraw - Draw to the left pane of the framebuffer
///
/// # Arguments
/// * `cmd_ptr` - Pointer to userspace FbDrawCmd structure
///
/// # Returns
/// * 0 on success
/// * -EFAULT if cmd_ptr is invalid
/// * -ENODEV if no framebuffer is available
/// * -EINVAL if operation is invalid
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub fn sys_fbdraw(cmd_ptr: u64) -> SyscallResult {
    // Validate pointer
    if cmd_ptr == 0 || cmd_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Validate the entire FbDrawCmd struct fits in userspace
    let end_ptr = cmd_ptr.saturating_add(core::mem::size_of::<FbDrawCmd>() as u64);
    if end_ptr > USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Read the command from userspace
    let cmd: FbDrawCmd = unsafe { core::ptr::read(cmd_ptr as *const FbDrawCmd) };

    // On ARM64, read fb_mmap info BEFORE acquiring SHELL_FRAMEBUFFER.
    // This prevents holding PROCESS_MANAGER (which disables interrupts on ARM64)
    // while also holding the framebuffer lock — that nested lock pattern caused
    // contention with the render thread and other syscall paths.
    #[cfg(target_arch = "aarch64")]
    let fb_mmap_info_pre: Option<crate::process::process::FbMmapInfo> = {
        use crate::syscall::memory_common::get_current_thread_id;
        let thread_id = get_current_thread_id();
        if let Some(tid) = thread_id {
            let mgr_guard = crate::process::manager();
            if let Some(ref mgr) = *mgr_guard {
                mgr.find_process_by_thread(tid)
                    .and_then(|(_pid, proc)| proc.fb_mmap)
            } else {
                None
            }
        } else {
            None
        }
    };

    // Get framebuffer
    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
    };

    // Use try_lock with bounded spin to avoid deadlocking with the render thread.
    // ARM64 syscalls run with DAIF=1111 (all interrupts disabled). If the render
    // thread holds this lock with interrupts enabled and gets preempted, a blocking
    // lock() here would spin forever — no timer interrupt can fire to reschedule
    // the render thread back. The render thread only holds the lock for brief pixel
    // ops (microseconds), so 4096 spins (~4μs) handles the common case. If the
    // lock holder was preempted, we return EBUSY and let userspace retry next frame.
    let mut fb_guard = {
        let mut guard = None;
        for _ in 0..4096 {
            if let Some(g) = fb.try_lock() {
                guard = Some(g);
                break;
            }
            core::hint::spin_loop();
        }
        match guard {
            Some(g) => g,
            None => return SyscallResult::Err(super::ErrorCode::Busy as u64),
        }
    };

    // Get left pane dimensions (half the screen width)
    let pane_width = fb_guard.width() / 2;
    let pane_height = fb_guard.height();

    // Parse color
    let color = Color::rgb(
        ((cmd.color >> 16) & 0xFF) as u8,
        ((cmd.color >> 8) & 0xFF) as u8,
        (cmd.color & 0xFF) as u8,
    );

    match cmd.op {
        0 => {
            // Clear: fill entire left pane with color
            fill_rect(
                &mut *fb_guard,
                Rect {
                    x: 0,
                    y: 0,
                    width: pane_width as u32,
                    height: pane_height as u32,
                },
                color,
            );
            #[cfg(target_arch = "aarch64")]
            crate::graphics::arm64_fb::mark_dirty(0, 0, pane_width as u32, pane_height as u32);
        }
        1 => {
            // FillRect: x, y, width, height, color
            // Clip to left pane
            let x = cmd.p1.max(0) as i32;
            let y = cmd.p2.max(0) as i32;
            let w = cmd.p3.max(0) as u32;
            let h = cmd.p4.max(0) as u32;

            // Only draw if within left pane
            if (x as usize) < pane_width {
                let clipped_w = w.min((pane_width as i32 - x) as u32);
                fill_rect(
                    &mut *fb_guard,
                    Rect { x, y, width: clipped_w, height: h },
                    color,
                );
                #[cfg(target_arch = "aarch64")]
                crate::graphics::arm64_fb::mark_dirty(x as u32, y as u32, clipped_w, h);
            }
        }
        2 => {
            // DrawRect: x, y, width, height, color
            let x = cmd.p1.max(0) as i32;
            let y = cmd.p2.max(0) as i32;
            let w = cmd.p3.max(0) as u32;
            let h = cmd.p4.max(0) as u32;

            if (x as usize) < pane_width {
                draw_rect(
                    &mut *fb_guard,
                    Rect { x, y, width: w, height: h },
                    color,
                );
                #[cfg(target_arch = "aarch64")]
                crate::graphics::arm64_fb::mark_dirty(x as u32, y as u32, w, h);
            }
        }
        3 => {
            // FillCircle: cx, cy, radius, color
            let cx = cmd.p1;
            let cy = cmd.p2;
            let radius = cmd.p3.max(0) as u32;

            if (cx as usize) < pane_width {
                fill_circle(&mut *fb_guard, cx, cy, radius, color);
                #[cfg(target_arch = "aarch64")]
                crate::graphics::arm64_fb::mark_dirty(
                    (cx - radius as i32).max(0) as u32,
                    (cy - radius as i32).max(0) as u32,
                    radius * 2,
                    radius * 2,
                );
            }
        }
        4 => {
            // DrawCircle: cx, cy, radius, color
            let cx = cmd.p1;
            let cy = cmd.p2;
            let radius = cmd.p3.max(0) as u32;

            if (cx as usize) < pane_width {
                draw_circle(&mut *fb_guard, cx, cy, radius, color);
                #[cfg(target_arch = "aarch64")]
                crate::graphics::arm64_fb::mark_dirty(
                    (cx - radius as i32).max(0) as u32,
                    (cy - radius as i32).max(0) as u32,
                    radius * 2,
                    radius * 2,
                );
            }
        }
        5 => {
            // DrawLine: x1, y1, x2, y2, color
            let x1 = cmd.p1;
            let y1 = cmd.p2;
            let x2 = cmd.p3;
            let y2 = cmd.p4;

            // Allow lines that start or end in left pane
            if (x1 as usize) < pane_width || (x2 as usize) < pane_width {
                draw_line(&mut *fb_guard, x1, y1, x2, y2, color);
                #[cfg(target_arch = "aarch64")]
                {
                    let min_x = x1.min(x2).max(0) as u32;
                    let min_y = y1.min(y2).max(0) as u32;
                    let max_x = x1.max(x2).max(0) as u32;
                    let max_y = y1.max(y2).max(0) as u32;
                    crate::graphics::arm64_fb::mark_dirty(
                        min_x, min_y,
                        max_x.saturating_sub(min_x) + 1,
                        max_y.saturating_sub(min_y) + 1,
                    );
                }
            }
        }
        6 => {
            // Flush: sync buffer to screen
            #[cfg(target_arch = "aarch64")]
            FB_FLUSH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // If p3 (width) and p4 (height) are non-zero, interpret p1-p4 as
            // a dirty rectangle (x, y, w, h) for partial flush. Otherwise,
            // fall back to full flush.
            let has_rect = cmd.p3 > 0 && cmd.p4 > 0;

            // If this process has an mmap'd framebuffer, copy user buffer → shadow
            // buffer's left pane before flushing.
            #[cfg(target_arch = "x86_64")]
            {
                // Check if this process has an fb_mmap by reading process state.
                // We read fb_mmap info while we already hold the FB lock, but since
                // fb_mmap is on the process (not the FB), we need the process manager lock.
                // To avoid deadlock, we drop the FB guard, read process info, then re-acquire.
                let fb_mmap_info = {
                    use crate::syscall::memory_common::get_current_thread_id;
                    let thread_id = get_current_thread_id();
                    if let Some(tid) = thread_id {
                        let mgr_guard = crate::process::manager();
                        if let Some(ref mgr) = *mgr_guard {
                            mgr.find_process_by_thread(tid)
                                .and_then(|(_pid, proc)| proc.fb_mmap)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                if let Some(mmap_info) = fb_mmap_info {
                    // Determine which rows to copy
                    let (y_start, y_end) = if has_rect {
                        let ys = (cmd.p2.max(0) as usize).min(mmap_info.height);
                        let ye = (cmd.p2.max(0) as usize + cmd.p4 as usize).min(mmap_info.height);
                        (ys, ye)
                    } else {
                        (0, mmap_info.height)
                    };

                    // Copy user buffer → shadow buffer at correct x_offset row by row
                    use crate::graphics::primitives::Canvas;
                    let fb_stride_bytes = fb_guard.stride() * fb_guard.bytes_per_pixel();
                    let row_bytes = mmap_info.width * mmap_info.bpp;
                    let x_byte_offset = mmap_info.x_offset * mmap_info.bpp;

                    if let Some(db) = fb_guard.double_buffer_mut() {
                        let shadow = db.buffer_mut();
                        for y in y_start..y_end {
                            let user_row_ptr = (mmap_info.user_addr as usize) + y * mmap_info.user_stride;
                            let shadow_row_offset = y * fb_stride_bytes + x_byte_offset;

                            if shadow_row_offset + row_bytes <= shadow.len() {
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        user_row_ptr as *const u8,
                                        shadow[shadow_row_offset..].as_mut_ptr(),
                                        row_bytes,
                                    );
                                }
                            }
                        }

                        // Mark dirty region and flush incrementally (in framebuffer coords)
                        let (dx_start, dx_end) = if has_rect {
                            let xs = mmap_info.x_offset + (cmd.p1.max(0) as usize).min(mmap_info.width);
                            let xe = mmap_info.x_offset + (cmd.p1.max(0) as usize + cmd.p3 as usize).min(mmap_info.width);
                            (xs, xe)
                        } else {
                            (mmap_info.x_offset, mmap_info.x_offset + mmap_info.width)
                        };
                        db.mark_region_dirty_rect(y_start, y_end, dx_start, dx_end);
                        db.flush();
                    }
                } else {
                    // No mmap: existing flush_full behavior
                    if let Some(db) = fb_guard.double_buffer_mut() {
                        db.flush_full();
                    }
                }
            }
            #[cfg(target_arch = "aarch64")]
            {
                // Use fb_mmap info that was read BEFORE acquiring SHELL_FRAMEBUFFER
                // to avoid holding PM (interrupts disabled) inside the FB lock.
                let fb_mmap_info = fb_mmap_info_pre;

                // Compute the flush rect BEFORE copying pixels (need mmap_info).
                let flush_rect: Option<(u32, u32, u32, u32)> = if let Some(mmap_info) = fb_mmap_info {
                    if has_rect {
                        Some((
                            (mmap_info.x_offset as u32) + cmd.p1.max(0) as u32,
                            cmd.p2.max(0) as u32,
                            cmd.p3 as u32,
                            cmd.p4 as u32,
                        ))
                    } else {
                        Some((
                            mmap_info.x_offset as u32,
                            0,
                            mmap_info.width as u32,
                            mmap_info.height as u32,
                        ))
                    }
                } else if has_rect {
                    Some((
                        cmd.p1.max(0) as u32,
                        cmd.p2.max(0) as u32,
                        cmd.p3 as u32,
                        cmd.p4 as u32,
                    ))
                } else {
                    // Full flush — get display dimensions
                    crate::drivers::virtio::gpu_mmio::dimensions()
                        .map(|(w, h)| (0u32, 0u32, w, h))
                };

                if let Some(mmap_info) = fb_mmap_info {
                    // Determine which rows to copy
                    let (y_start, y_end) = if has_rect {
                        let ys = (cmd.p2.max(0) as usize).min(mmap_info.height);
                        let ye = (cmd.p2.max(0) as usize + cmd.p4 as usize).min(mmap_info.height);
                        (ys, ye)
                    } else {
                        (0, mmap_info.height)
                    };

                    // Copy dirty rows from user buffer → GPU framebuffer at correct x_offset.
                    // ARM64 has no double buffer; writes go directly to VirtIO GPU memory.
                    let fb_stride_bytes = fb_guard.stride() * fb_guard.bytes_per_pixel();
                    let row_bytes = mmap_info.width * mmap_info.bpp;
                    let x_byte_offset = mmap_info.x_offset * mmap_info.bpp;
                    let gpu_buf = fb_guard.buffer_mut();

                    for y in y_start..y_end {
                        let user_row_ptr = (mmap_info.user_addr as usize) + y * mmap_info.user_stride;
                        let gpu_row_offset = y * fb_stride_bytes + x_byte_offset;

                        if gpu_row_offset + row_bytes <= gpu_buf.len() {
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    user_row_ptr as *const u8,
                                    gpu_buf[gpu_row_offset..].as_mut_ptr(),
                                    row_bytes,
                                );
                            }
                        }
                    }
                }

                // Drop SHELL_FRAMEBUFFER lock BEFORE GPU flush to avoid holding
                // both SHELL_FRAMEBUFFER + GPU_LOCK simultaneously. The pixel
                // copy is done; the render thread can now access the framebuffer
                // for terminal text while we submit GPU commands.
                drop(fb_guard);

                // Synchronous GPU flush — submit transfer_to_host + resource_flush
                // directly in the syscall instead of deferring to the render thread.
                // This eliminates scheduling latency: bounce's frame is displayed
                // immediately rather than waiting for the render thread to wake up
                // (which could take 5ms+ due to timer tick granularity).
                if let Some((fx, fy, fw, fh)) = flush_rect {
                    let _ = crate::drivers::virtio::gpu_mmio::flush_rect(fx, fy, fw, fh);
                }
            }
        }
        _ => {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
    }

    SyscallResult::Ok(0)
}

/// sys_fbdraw - Stub for non-interactive mode
#[cfg(not(any(target_arch = "aarch64", feature = "interactive")))]
pub fn sys_fbdraw(_cmd_ptr: u64) -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// sys_get_mouse_pos - Get current mouse cursor position and button state
///
/// # Arguments
/// * `out_ptr` - Pointer to a [u32; 3] array in userspace: [x, y, buttons]
///   buttons: bit 0 = left button pressed
///
/// # Returns
/// * 0 on success
/// * -EFAULT if out_ptr is invalid
#[cfg(target_arch = "aarch64")]
pub fn sys_get_mouse_pos(out_ptr: u64) -> SyscallResult {
    if out_ptr == 0 || out_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    let end_ptr = out_ptr.saturating_add(12); // 3 * u32
    if end_ptr > USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    let (mx, my, buttons) = crate::drivers::virtio::input_mmio::mouse_state();

    unsafe {
        let out = out_ptr as *mut [u32; 3];
        core::ptr::write(out, [mx, my, buttons]);
    }

    SyscallResult::Ok(0)
}

/// sys_get_mouse_pos - Stub for non-aarch64
#[cfg(not(target_arch = "aarch64"))]
pub fn sys_get_mouse_pos(_out_ptr: u64) -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// sys_fbmmap - Map a framebuffer buffer into the calling process's address space
///
/// Allocates physical frames, maps them into the process as a compact left-pane
/// buffer, and returns the userspace pointer. Drawing can then happen with zero
/// syscalls; only the flush requires a syscall.
///
/// # Returns
/// * Userspace address of the mapped buffer on success
/// * -EBUSY if already mapped
/// * -ENOMEM if allocation fails
/// * -ENODEV if no framebuffer is available
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub fn sys_fbmmap() -> SyscallResult {
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        cleanup_mapped_pages, flush_tlb, get_current_thread_id, prot_to_page_flags,
        round_down_to_page, round_up_to_page, PAGE_SIZE,
    };
    #[cfg(target_arch = "x86_64")]
    use x86_64::structures::paging::{Page, Size4KiB};
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};

    // Get current process thread ID first (needed for per-process display ownership check)
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    // Check if the calling process owns the display (called take_over_display)
    let caller_owns_display = {
        let mgr_guard = crate::process::manager();
        if let Some(ref mgr) = *mgr_guard {
            mgr.find_process_by_thread(current_thread_id)
                .map(|(_pid, proc)| proc.has_display_ownership)
                .unwrap_or(false)
        } else {
            false
        }
    };

    // Get framebuffer dimensions (acquire and release FB lock quickly)
    // The display owner (BWM) gets the right pane. All other processes get the left pane.
    let (pane_width, x_offset, height, bpp) = {
        let fb = match SHELL_FRAMEBUFFER.get() {
            Some(fb) => fb,
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        };
        // Use try_lock with bounded spin (same rationale as sys_fbinfo).
        let fb_guard = {
            let mut guard = None;
            for _ in 0..65536 {
                if let Some(g) = fb.try_lock() {
                    guard = Some(g);
                    break;
                }
                core::hint::spin_loop();
            }
            match guard {
                Some(g) => g,
                None => {
                    log::warn!("sys_fbmmap: framebuffer lock busy");
                    return SyscallResult::Err(super::ErrorCode::Busy as u64);
                }
            }
        };
        if caller_owns_display {
            // BWM mode: right half for window manager (after divider)
            let divider_width = 4;
            let right_x = fb_guard.width() / 2 + divider_width;
            let right_width = fb_guard.width().saturating_sub(right_x);
            (right_width, right_x, fb_guard.height(), fb_guard.bytes_per_pixel())
        } else {
            // Normal mode: left half for graphics demos
            (fb_guard.width() / 2, 0, fb_guard.height(), fb_guard.bytes_per_pixel())
        }
    };

    let user_stride = pane_width * bpp;
    let buf_size = (user_stride * height) as u64;
    let mapping_size = round_up_to_page(buf_size);

    // Phase 1: Quick PM acquisition — reserve address range and check preconditions.
    // Release PM before the expensive frame allocation + zeroing.
    let (start_addr, end_addr) = {
        let mut manager_guard = crate::process::manager();
        let manager = match *manager_guard {
            Some(ref mut m) => m,
            None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
        };

        let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
        };

        if process.fb_mmap.is_some() {
            return SyscallResult::Err(super::ErrorCode::Busy as u64);
        }

        let new_addr = round_down_to_page(process.mmap_hint.saturating_sub(mapping_size));
        if new_addr < 0x1000_0000 {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        process.mmap_hint = new_addr;

        (new_addr, new_addr + mapping_size)
    }; // PM released — other threads can dispatch with TTBR0

    // Phase 2: Pre-allocate and zero all frames WITHOUT holding PM.
    // This is the expensive part (~500 frames × 4KB zero = ~2MB memset).
    let physical_memory_offset = crate::memory::physical_memory_offset();
    let start_page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_addr));
    let end_page = Page::<Size4KiB>::containing_address(VirtAddr::new(end_addr - 1));

    let mut frames = alloc::vec::Vec::new();
    {
        let mut page = start_page;
        loop {
            let frame = match crate::memory::frame_allocator::allocate_frame() {
                Some(f) => f,
                None => {
                    for f in &frames {
                        crate::memory::frame_allocator::deallocate_frame(*f);
                    }
                    return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
                }
            };

            // Zero via physical address (no page table or PM needed)
            let phys_addr = frame.start_address().as_u64();
            let virt_ptr = (physical_memory_offset.as_u64() + phys_addr) as *mut u8;
            unsafe {
                core::ptr::write_bytes(virt_ptr, 0, PAGE_SIZE as usize);
            }

            frames.push(frame);
            if page >= end_page {
                break;
            }
            page += 1;
        }
    }

    // Phase 3: Quick PM acquisition — map pre-allocated frames into page table.
    // Only page table entry writes + TLB flushes here (fast).
    let mut manager_guard = crate::process::manager();
    let manager = match *manager_guard {
        Some(ref mut m) => m,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
        }
    };

    let (_pid, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64);
        }
    };

    let page_table = match process.page_table.as_mut() {
        Some(pt) => pt,
        None => {
            for f in &frames {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
    };

    let prot = Protection::from_bits_truncate(3);
    let page_flags = prot_to_page_flags(prot);
    let mut mapped_pages = alloc::vec::Vec::new();
    let mut current_page = start_page;

    for frame in &frames {
        if let Err(_e) = page_table.map_page(current_page, *frame, page_flags) {
            cleanup_mapped_pages(page_table, &mapped_pages);
            // Deallocate remaining unmapped frames
            for f in frames.iter().skip(mapped_pages.len()) {
                crate::memory::frame_allocator::deallocate_frame(*f);
            }
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }

        flush_tlb(current_page.start_address());
        mapped_pages.push((current_page, *frame));

        if current_page >= end_page {
            break;
        }
        current_page += 1;
    }

    // Create VMA (MAP_ANONYMOUS=0x20 | MAP_PRIVATE=0x02 = 0x22)
    let vma = Vma::new(
        VirtAddr::new(start_addr),
        VirtAddr::new(end_addr),
        prot,
        MmapFlags::from_bits_truncate(0x22),
    );
    process.vmas.push(vma);

    // Store FbMmapInfo
    process.fb_mmap = Some(crate::process::process::FbMmapInfo {
        user_addr: start_addr,
        width: pane_width,
        height,
        user_stride,
        bpp,
        mapping_size,
        x_offset,
    });

    log::info!(
        "sys_fbmmap: mapped {}x{} fb buffer at {:#x} ({} pages)",
        pane_width, height, start_addr, mapped_pages.len()
    );

    SyscallResult::Ok(start_addr)
}

/// sys_fbmmap - Stub for non-interactive mode
#[cfg(not(any(target_arch = "aarch64", feature = "interactive")))]
pub fn sys_fbmmap() -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}
