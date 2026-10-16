//! The framebuffer system calls: `fbinfo` (410), `fb_mmap` (412), `get_mouse_pos` and the
//! framebuffer operations of `fb_draw` (411) that touch the display: `FLUSH` (6) and
//! `FLUSH_BATCH` (8). `fb_draw`'s op table lives in `fb_draw.rs`.

extern crate alloc;

// Architecture-specific framebuffer imports
#[cfg(target_arch = "aarch64")]
use crate::graphics::arm64_fb::SHELL_FRAMEBUFFER;
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
use crate::logger::SHELL_FRAMEBUFFER;

use super::fb_draw::FbDrawCmd;
use super::SyscallResult;
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
use crate::graphics::primitives::Canvas;

/// Counter for fb_flush syscalls (diagnostic — read from timer heartbeat)
#[cfg(target_arch = "aarch64")]
pub static FB_FLUSH_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PID of the process that most recently took the display
/// (`sys_take_over_display`). One process owns the display at a time: a later
/// take-over moves ownership there, and the earlier owner's whole-screen
/// mapping can no longer draw.
pub(crate) static DISPLAY_OWNER_PID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);

/// Whether `process` (whose PID is `pid`) owns the display now.
fn owns_display(
    pid: crate::process::process::ProcessId,
    process: &crate::process::process::Process,
) -> bool {
    process.has_display_ownership
        && DISPLAY_OWNER_PID.load(core::sync::atomic::Ordering::Acquire) == pid.as_u64()
}

#[derive(Clone, Copy)]
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
pub(super) const USER_SPACE_MAX: u64 = crate::memory::layout::USER_STACK_REGION_END;

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

/// sys_fbinfo - production x86_64 form: the bootloader framebuffer the kernel
/// logs to until a process takes the display.
#[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
pub fn sys_fbinfo(info_ptr: u64) -> SyscallResult {
    let Some((width, height, stride, bpp, is_bgr)) = crate::logger::display_geometry() else {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    };
    let info = FbInfo {
        width: width as u64,
        height: height as u64,
        stride: stride as u64,
        bytes_per_pixel: bpp as u64,
        pixel_format: if is_bgr { 1 } else { 0 },
    };
    match crate::syscall::userptr::copy_to_user(info_ptr as *mut FbInfo, &info) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(super::ErrorCode::Fault as u64),
    }
}

/// The `fb_draw` ops that flush the framebuffer: `FLUSH` (6) and `FLUSH_BATCH` (8).
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub(super) fn framebuffer_op(cmd: FbDrawCmd) -> SyscallResult {
    // Read the caller's fb_mmap info and display ownership BEFORE acquiring
    // SHELL_FRAMEBUFFER. This prevents holding PROCESS_MANAGER (which disables
    // interrupts on ARM64) while also holding the framebuffer lock — that nested
    // lock pattern caused contention with the render thread and other syscall paths.
    let (fb_mmap_info_pre, caller_owns_display): (
        Option<crate::process::process::FbMmapInfo>,
        bool,
    ) = {
        use crate::syscall::memory_common::get_current_thread_id;
        let thread_id = get_current_thread_id();
        if let Some(tid) = thread_id {
            let mgr_guard = crate::process::manager();
            if let Some(ref mgr) = *mgr_guard {
                mgr.find_process_by_thread(tid)
                    .map(|(pid, proc)| (proc.fb_mmap, owns_display(pid, proc)))
                    .unwrap_or((None, false))
            } else {
                (None, false)
            }
        } else {
            (None, false)
        }
    };

    // A whole-screen mapping was made by a display owner. Once another process
    // has taken the display, drawing through it would overwrite the new owner's
    // frames, so the caller is told with EPERM (unlike the transient EBUSY below).
    if fb_mmap_info_pre.is_some_and(|info| info.whole_screen) && !caller_owns_display {
        return SyscallResult::Err(super::ErrorCode::PermissionDenied as u64);
    }

    #[cfg(target_arch = "aarch64")]
    if crate::graphics::display::active() && cmd.op == 6 {
        if let (Some(pane),Ok(caller))=(fb_mmap_info_pre,crate::window::Caller::current()) {
            let rect=if cmd.p3>0 && cmd.p4>0 {breengel_abi::Rect::new(cmd.p1,cmd.p2,cmd.p3,cmd.p4)}
                else {breengel_abi::Rect::new(0,0,pane.width as i32,pane.height as i32)};
            return match crate::graphics::display::flush(caller.tgid,pane.user_addr,pane.user_stride,rect) {
                Ok(())=>SyscallResult::Ok(0),Err(e)=>SyscallResult::Err(e as u64)
            };
        }
    }

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

    // Every op that reaches here (6, 8) puts pixels on this framebuffer: userspace
    // is drawing, so the kernel's boot screen gives the screen up, under the same
    // lock as this draw. After a kernel failure the diagnostics screen keeps it.
    #[cfg(target_arch = "aarch64")]
    if !crate::graphics::boot_screen::hand_over(&mut fb_guard) {
        return SyscallResult::Err(super::ErrorCode::Busy as u64);
    }

    match cmd.op {
        6 => {
            // Flush: sync buffer to screen
            #[cfg(target_arch = "aarch64")]
            FB_FLUSH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // If p3 (width) and p4 (height) are non-zero, interpret p1-p4 as
            // a dirty rectangle (x, y, w, h) for partial flush. Otherwise,
            // fall back to full flush.
            let has_rect = cmd.p3 > 0 && cmd.p4 > 0;

            // If this process has an mmap'd framebuffer, copy user buffer → shadow
            // buffer (its pane starts at x 0) before flushing.
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

                    // Copy user buffer → shadow buffer row by row
                    use crate::graphics::primitives::Canvas;
                    let fb_stride_bytes = fb_guard.stride() * fb_guard.bytes_per_pixel();
                    let row_bytes = mmap_info.width * mmap_info.bpp;

                    if let Some(db) = fb_guard.double_buffer_mut() {
                        let shadow = db.buffer_mut();
                        for y in y_start..y_end {
                            let user_row_ptr =
                                (mmap_info.user_addr as usize) + y * mmap_info.user_stride;
                            let shadow_row_offset = y * fb_stride_bytes;

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
                            let xs = (cmd.p1.max(0) as usize).min(mmap_info.width);
                            let xe =
                                (cmd.p1.max(0) as usize + cmd.p3 as usize).min(mmap_info.width);
                            (xs, xe)
                        } else {
                            (0, mmap_info.width)
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
                let flush_rect: Option<(u32, u32, u32, u32)> = if let Some(mmap_info) = fb_mmap_info
                {
                    if has_rect {
                        Some((
                            cmd.p1.max(0) as u32,
                            cmd.p2.max(0) as u32,
                            cmd.p3 as u32,
                            cmd.p4 as u32,
                        ))
                    } else {
                        Some((0, 0, mmap_info.width as u32, mmap_info.height as u32))
                    }
                } else if has_rect {
                    Some((
                        cmd.p1.max(0) as u32,
                        cmd.p2.max(0) as u32,
                        cmd.p3 as u32,
                        cmd.p4 as u32,
                    ))
                } else {
                    // Full flush — get display dimensions.
                    // Check FB_INFO_CACHE first (works for GOP and all backends),
                    // then fall back to gpu_mmio::dimensions() for QEMU.
                    crate::graphics::arm64_fb::FB_INFO_CACHE
                        .get()
                        .map(|c| (0u32, 0u32, c.width as u32, c.height as u32))
                        .or_else(|| {
                            crate::drivers::virtio::gpu_mmio::dimensions()
                                .map(|(w, h)| (0u32, 0u32, w, h))
                        })
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

                    let fb_stride_bytes = fb_guard.stride() * fb_guard.bytes_per_pixel();
                    let row_bytes = mmap_info.width * mmap_info.bpp;

                    // When a dirty rect is specified, only copy the dirty columns
                    // instead of the full mmap width. For per-ball flushes this
                    // reduces the copy from ~3.4KB/row to ~336 bytes/row.
                    let (col_offset, copy_row_bytes) = if has_rect {
                        let col_start = (cmd.p1.max(0) as usize).min(mmap_info.width);
                        let col_end =
                            (cmd.p1.max(0) as usize + cmd.p3 as usize).min(mmap_info.width);
                        (
                            col_start * mmap_info.bpp,
                            (col_end - col_start) * mmap_info.bpp,
                        )
                    } else {
                        (0, row_bytes)
                    };

                    if crate::graphics::arm64_fb::is_gop_active() {
                        // GOP synchronous path: copy mmap → BAR0 directly with
                        // partial column copy. Each per-ball flush writes only
                        // ~27KB to BAR0 instead of the full bounding box (~3.7MB).
                        // Also update shadow buffer for consistency with terminal text.
                        //
                        // VirtIO DMA (PCI_FRAMEBUFFER → TRANSFER_TO_HOST_2D) was
                        // benchmarked and is slower: 5-7 FPS per-ball, 4-8 FPS
                        // full-pane, vs 12 FPS with direct BAR0 MMIO.
                        if let Some(gop_buf) = crate::graphics::arm64_fb::gop_framebuffer() {
                            for y in y_start..y_end {
                                let user_row_ptr = (mmap_info.user_addr as usize)
                                    + y * mmap_info.user_stride
                                    + col_offset;
                                let target_row_offset = y * fb_stride_bytes + col_offset;
                                if target_row_offset + copy_row_bytes <= gop_buf.len() {
                                    unsafe {
                                        core::ptr::copy_nonoverlapping(
                                            user_row_ptr as *const u8,
                                            gop_buf[target_row_offset..].as_mut_ptr(),
                                            copy_row_bytes,
                                        );
                                    }
                                }
                            }
                        }
                        // Update shadow buffer so terminal reads stay consistent
                        if let Some(db) = fb_guard.double_buffer_mut() {
                            let shadow = db.buffer_mut();
                            for y in y_start..y_end {
                                let user_row_ptr = (mmap_info.user_addr as usize)
                                    + y * mmap_info.user_stride
                                    + col_offset;
                                let target_row_offset = y * fb_stride_bytes + col_offset;
                                if target_row_offset + copy_row_bytes <= shadow.len() {
                                    unsafe {
                                        core::ptr::copy_nonoverlapping(
                                            user_row_ptr as *const u8,
                                            shadow[target_row_offset..].as_mut_ptr(),
                                            copy_row_bytes,
                                        );
                                    }
                                }
                            }
                        }
                    } else {
                        // Non-GOP path: copy to GPU buffer (VirtIO MMIO/PCI framebuffer)
                        let target_buf = fb_guard.buffer_mut();
                        for y in y_start..y_end {
                            let user_row_ptr =
                                (mmap_info.user_addr as usize) + y * mmap_info.user_stride;
                            let target_row_offset = y * fb_stride_bytes;

                            if target_row_offset + row_bytes <= target_buf.len() {
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        user_row_ptr as *const u8,
                                        target_buf[target_row_offset..].as_mut_ptr(),
                                        row_bytes,
                                    );
                                }
                            }
                        }
                    }
                }

                // Drop SHELL_FRAMEBUFFER lock before GPU flush
                drop(fb_guard);

                // Synchronous GPU flush — for GOP this is a DSB barrier ensuring
                // BAR0 writes are visible to the display controller. For VirtIO
                // this submits transfer_to_host + resource_flush.
                if let Some((fx, fy, fw, fh)) = flush_rect {
                    let _ = crate::graphics::arm64_fb::flush_dirty_rect(fx, fy, fw, fh);
                }
            }
        }
        8 => {
            // FlushBatch: batch flush multiple dirty rects with one DSB barrier.
            // p1:p2 = 64-bit pointer to FlushRect array [(x, y, w, h); ...]
            // p3 = count of rects (max 16)
            // Copies each rect from mmap → BAR0, then ONE dsb sy.
            // Saves 12+ syscall round-trips and DSB barriers per frame.
            #[cfg(target_arch = "aarch64")]
            {
                FB_FLUSH_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

                let rects_ptr = (cmd.p1 as u32 as u64) | ((cmd.p2 as u32 as u64) << 32);
                let count = (cmd.p3 as u32).min(16) as usize;

                // Drop FB lock immediately — batch flush only needs mmap_info + BAR0
                drop(fb_guard);

                if count == 0 {
                    return SyscallResult::Ok(0);
                }

                if rects_ptr == 0 || rects_ptr >= USER_SPACE_MAX {
                    return SyscallResult::Err(super::ErrorCode::Fault as u64);
                }
                let rects_end = rects_ptr.saturating_add((count as u64) * 16);
                if rects_end > USER_SPACE_MAX {
                    return SyscallResult::Err(super::ErrorCode::Fault as u64);
                }

                #[repr(C)]
                #[derive(Clone, Copy)]
                struct FlushRect {
                    x: i32,
                    y: i32,
                    w: i32,
                    h: i32,
                }

                let rects =
                    unsafe { core::slice::from_raw_parts(rects_ptr as *const FlushRect, count) };

                let fb_mmap_info = fb_mmap_info_pre;

                if let Some(mmap_info) = fb_mmap_info {
                    if crate::graphics::arm64_fb::is_gop_active() {
                        // Use lock-free FbInfoCache for stride (no FB lock needed)
                        let fb_stride_bytes = crate::graphics::arm64_fb::FB_INFO_CACHE
                            .get()
                            .map(|c| c.stride * c.bytes_per_pixel)
                            .unwrap_or(0);

                        if fb_stride_bytes > 0 {
                            if let Some(gop_buf) = crate::graphics::arm64_fb::gop_framebuffer() {
                                for rect in rects {
                                    if rect.w <= 0 || rect.h <= 0 {
                                        continue;
                                    }

                                    let col_start = (rect.x.max(0) as usize).min(mmap_info.width);
                                    let col_end = (rect.x.max(0) as usize + rect.w as usize)
                                        .min(mmap_info.width);
                                    let y_start = (rect.y.max(0) as usize).min(mmap_info.height);
                                    let y_end = (rect.y.max(0) as usize + rect.h as usize)
                                        .min(mmap_info.height);

                                    let col_byte = col_start * mmap_info.bpp;
                                    let copy_row_bytes = (col_end - col_start) * mmap_info.bpp;

                                    if copy_row_bytes == 0 {
                                        continue;
                                    }

                                    for y in y_start..y_end {
                                        let user_row_ptr = (mmap_info.user_addr as usize)
                                            + y * mmap_info.user_stride
                                            + col_byte;
                                        let target_row_offset = y * fb_stride_bytes + col_byte;
                                        if target_row_offset + copy_row_bytes <= gop_buf.len() {
                                            unsafe {
                                                core::ptr::copy_nonoverlapping(
                                                    user_row_ptr as *const u8,
                                                    gop_buf[target_row_offset..].as_mut_ptr(),
                                                    copy_row_bytes,
                                                );
                                            }
                                        }
                                    }
                                }
                            }

                            // ONE DSB for all BAR0 writes
                            unsafe {
                                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
                            }
                        }
                    }
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                drop(fb_guard);
                return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
            }
        }
        _ => {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
    }

    SyscallResult::Ok(0)
}

/// `FLUSH` (op 6) on the production x86_64 kernel: copies the display owner's whole-screen
/// fb_mmap pane (or the rectangle p1-p4 of it, when p3 and p4 are non-zero) onto the screen.
#[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
pub(super) fn flush_owner_pane(cmd: &FbDrawCmd) -> SyscallResult {
    use crate::syscall::memory_common::get_current_thread_id;

    let (fb_mmap, caller_owns_display) = match get_current_thread_id() {
        Some(tid) => match *crate::process::manager() {
            Some(ref mgr) => mgr
                .find_process_by_thread(tid)
                .map(|(pid, proc)| (proc.fb_mmap, owns_display(pid, proc)))
                .unwrap_or((None, false)),
            None => (None, false),
        },
        None => (None, false),
    };
    if !caller_owns_display {
        return SyscallResult::Err(super::ErrorCode::PermissionDenied as u64);
    }
    let Some(pane) = fb_mmap.filter(|pane| pane.whole_screen) else {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    };
    let (rows, columns) = if cmd.p3 > 0 && cmd.p4 > 0 {
        let (x, y) = (cmd.p1.max(0) as usize, cmd.p2.max(0) as usize);
        (
            y.min(pane.height)..(y + cmd.p4 as usize).min(pane.height),
            x.min(pane.width)..(x + cmd.p3 as usize).min(pane.width),
        )
    } else {
        (0..pane.height, 0..pane.width)
    };
    // SAFETY: the pane is the caller's own fb_mmap mapping, `pane.height` rows of
    // `pane.user_stride` bytes, mapped with pre-allocated frames, and the copy is
    // clipped to it.
    let copied = unsafe {
        crate::logger::copy_to_display(pane.user_addr as *const u8, pane.user_stride, rows, columns)
    };
    if copied {
        SyscallResult::Ok(0)
    } else {
        SyscallResult::Err(super::ErrorCode::Busy as u64)
    }
}

/// sys_get_mouse_pos - Get current mouse cursor position, button state, and scroll delta
///
/// # Arguments
/// * `out_ptr` - Pointer to a [u32; 4] array in userspace: [x, y, buttons, scroll_delta]
///   buttons: bit 0 = left button pressed
///   scroll_delta: accumulated scroll wheel ticks since last call (positive = up)
///
/// # Returns
/// * 0 on success
/// * -EFAULT if out_ptr is invalid
#[cfg(target_arch = "aarch64")]
pub fn sys_get_mouse_pos(out_ptr: u64) -> SyscallResult {
    // The state below is consumed as it is read: check the buffer first, so a bad one
    // loses nothing.
    if crate::window::mem::probe_writable(out_ptr, 16).is_err() {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }
    let (mx, my, buttons) = if crate::drivers::virtio::input_mmio::is_tablet_initialized() {
        crate::drivers::virtio::input_mmio::mouse_state()
    } else {
        crate::drivers::usb::hid::mouse_state_consume()
    };

    let scroll = crate::drivers::usb::hid::mouse_wheel_consume() as u32;

    // Through the fault-tolerant copy, so an unmapped or read-only buffer is EFAULT.
    match crate::syscall::userptr::copy_to_user(out_ptr as *mut [u32; 4], &[mx, my, buttons, scroll]) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(super::ErrorCode::Fault as u64),
    }
}

/// sys_get_mouse_pos - Stub for non-aarch64
#[cfg(not(target_arch = "aarch64"))]
pub fn sys_get_mouse_pos(_out_ptr: u64) -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// sys_fbmmap - Map a framebuffer buffer into the calling process's address space
///
/// Allocates physical frames, maps them into the process as a compact buffer at
/// x 0 (the whole screen for the display owner, the left half for everyone
/// else), and returns the userspace pointer. Drawing then needs no syscall;
/// only the flush requires one. When the argument `width_out` is not 0 it
/// receives the pane width in pixels (the row stride is that width times bytes
/// per pixel).
///
/// # Returns
/// * Userspace address of the mapped buffer on success
/// * -EBUSY if already mapped
/// * -ENOMEM if allocation fails
/// * -ENODEV if no framebuffer is available
#[cfg(target_arch = "aarch64")]
pub fn sys_fbmmap(width_out: u64) -> SyscallResult {
    fbmmap(width_out)
}

/// sys_fbmmap - x86_64 form; see the aarch64 form above.
#[cfg(target_arch = "x86_64")]
pub fn sys_fbmmap(width_out: u64) -> SyscallResult {
    fbmmap(width_out)
}

fn fbmmap(width_out: u64) -> SyscallResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::{Page, Size4KiB, VirtAddr};
    use crate::memory::vma::{MmapFlags, Protection, Vma};
    use crate::syscall::memory_common::{
        cleanup_mapped_pages, flush_new_mapping, get_current_thread_id, place_below_hint,
        prot_to_page_flags, round_up_to_page, PAGE_SIZE,
    };
    #[cfg(target_arch = "x86_64")]
    use x86_64::structures::paging::{Page, Size4KiB};
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    // Get current process thread ID first (needed for per-process display ownership check)
    let current_thread_id = match get_current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::ErrorCode::NoSuchProcess as u64),
    };

    // Check the optional pane-width out pointer before mapping anything.
    if width_out != 0
        && crate::syscall::userptr::validate_user_ptr_write(width_out as *mut u64).is_err()
    {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Check if the calling process owns the display (called take_over_display)
    let caller_owns_display = {
        let mgr_guard = crate::process::manager();
        if let Some(ref mgr) = *mgr_guard {
            mgr.find_process_by_thread(current_thread_id)
                .map(|(pid, proc)| owns_display(pid, proc))
                .unwrap_or(false)
        } else {
            false
        }
    };

    // Get framebuffer dimensions.
    // The display owner (the process that called take_over_display) gets the
    // whole screen. Other processes get the left half.
    let whole_screen = caller_owns_display;
    //
    // On ARM64, use the lock-free FbInfoCache to avoid contention with BWM's
    // fb_flush, which holds SHELL_FRAMEBUFFER for ~400μs during full-screen
    // pixel copies. Dimensions are immutable after init.
    #[cfg(target_arch = "aarch64")]
    let (pane_width, height, bpp) = {
        let cache = match crate::graphics::arm64_fb::FB_INFO_CACHE.get() {
            Some(c) => c,
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
        };
        let pane_width = if whole_screen {
            cache.width
        } else {
            cache.width / 2
        };
        (pane_width, cache.height, cache.bytes_per_pixel)
    };

    #[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
    let (pane_width, height, bpp) = {
        let Some((width, height, _stride, bpp, _is_bgr)) = crate::logger::display_geometry() else {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        };
        let pane_width = if whole_screen { width } else { width / 2 };
        (pane_width, height, bpp)
    };

    #[cfg(all(target_arch = "x86_64", feature = "interactive"))]
    let (pane_width, height, bpp) = {
        let fb = match SHELL_FRAMEBUFFER.get() {
            Some(fb) => fb,
            None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
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
                    log::warn!("sys_fbmmap: framebuffer lock busy");
                    return SyscallResult::Err(super::ErrorCode::Busy as u64);
                }
            }
        };
        let pane_width = if whole_screen {
            fb_guard.width()
        } else {
            fb_guard.width() / 2
        };
        (pane_width, fb_guard.height(), fb_guard.bytes_per_pixel())
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

        if process.mapped_bytes().saturating_add(mapping_size)
            > process.limits.get(crate::process::limits::AS).soft
        {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        }
        let Some(new_addr) = place_below_hint(process, mapping_size) else {
            return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
        };

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

        flush_new_mapping(current_page.start_address());
        mapped_pages.push((current_page, *frame));

        if current_page >= end_page {
            break;
        }
        current_page += 1;
    }

    // MAP_SHARED | MAP_ANONYMOUS, as the window and texture buffers above are:
    // a fork shares the pane's frames instead of write-protecting them. As a
    // private mapping, every fork made the display owner fault on, and copy or
    // re-own, each pane page it drew next, under PROCESS_MANAGER.
    let vma = Vma::new(
        VirtAddr::new(start_addr),
        VirtAddr::new(end_addr),
        prot,
        MmapFlags::from_bits_truncate(0x21),
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
        whole_screen,
    });
    drop(manager_guard);

    // The pointer was validated before mapping.
    if width_out != 0 {
        let _ = crate::syscall::userptr::copy_to_user(width_out as *mut u64, &(pane_width as u64));
    }

    log::info!(
        "sys_fbmmap: mapped {}x{} fb buffer at {:#x} ({} pages)",
        pane_width,
        height,
        start_addr,
        mapped_pages.len()
    );

    SyscallResult::Ok(start_addr)
}
