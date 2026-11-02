//! The x86 framebuffer ABI's private scanout after native GPU admission.
//! Resources and contexts use the caller's ordinary process/DMA lifetimes.
use super::*;
use core::ops::Range;
use crate::process::process::FbMmapInfo;

#[derive(Clone, Copy)]
pub(super) struct Pane {
    context: u64,
    ctx: u32,
    handle: u64,
    id: u32,
    width: usize,
    height: usize,
}

fn target(owner: u64, width: usize, height: usize) -> Result<Pane, i64> {
    if let Some(pane) = with_core(|c| c.proc_mut(owner).framebuffer).flatten() {
        return if (pane.width, pane.height) == (width, height) { Ok(pane) } else { Err(EINVAL) };
    }
    let (context, _) = create_context_with_catalogue(owner, false)?;
    let made = (|| {
        let _creation = CreationGuard::acquire(owner)?;
        if let Some(pane) = with_core(|c| c.proc_mut(owner).framebuffer).flatten() {
            return Ok((pane, false));
        }
        if !with_core(|c| c.proc_mut(owner).resources.len() < RESOURCE_QUOTA).unwrap_or(false) {
            return Err(EAGAIN);
        }
        let ctx = with_core(|c| c.ctx_of(owner, context)).flatten().ok_or(EIO)?;
        let pitch = (width * 4).next_multiple_of(256);
        let pages = device_frames(owner, pitch.checked_mul(height).ok_or(EINVAL)?)?;
        let id = console::resource_with(ctx, kind::TARGET, format::B8G8R8A8,
            width as u32, height as u32, true, pages)?;
        let pane = with_core(|c| {
            let handle = c.handle();
            let resource = c.resources.get_mut(&id).ok_or(EIO)?;
            resource.owner = owner;
            c.proc_mut(owner).resources.insert(handle, id);
            let pane = Pane { context, ctx, handle, id, width, height };
            c.proc_mut(owner).framebuffer = Some(pane);
            // The process handle now owns it; requests add their own DMA holds.
            c.unhold(id);
            Ok::<_, i64>(pane)
        }).ok_or(ENODEV)??;
        Ok((pane, true))
    })();
    match made {
        Ok((pane, true)) => Ok(pane),
        Ok((pane, false)) => { destroy_context(owner, context); Ok(pane) }
        Err(error) => { destroy_context(owner, context); Err(error) }
    }
}

/// Copy only validated caller bytes, using bounded stack staging and the target's pitch.
fn copy_rows(pages: &Pages, pitch: usize, pane: FbMmapInfo,
    rows: Range<usize>, columns: Range<usize>) -> Result<(), i64> {
    let mut chunk = [0u8; 512];
    let start = columns.start * 4;
    let end = columns.end * 4;
    for y in rows {
        for at in (start..end).step_by(chunk.len()) {
            let n = chunk.len().min(end - at);
            crate::syscall::userptr::read_user_bytes(chunk.as_mut_ptr(),
                pane.user_addr + (y * pane.user_stride + at) as u64, n).map_err(|_| EFAULT)?;
            // The legacy framebuffer is BGRX; GPU targets have opaque alpha.
            for pixel in chunk[..n].chunks_exact_mut(4) { pixel[3] = 255; }
            pages.write(y * pitch + at, &chunk[..n]);
        }
    }
    Ok(())
}

pub fn flush(pane: FbMmapInfo, mut rows: Range<usize>, mut columns: Range<usize>) -> Result<(), i64> {
    let _path = GpuPath::enter();
    let owner = caller()?;
    if !display::owner(owner) { return Err(abi::errno::EPERM); }
    if pane.bpp != 4 || !pane.whole_screen || pane.user_stride < pane.width * 4
        || rows.end > pane.height || columns.end > pane.width {
        return Err(EINVAL);
    }
    if rows.is_empty() || columns.is_empty() { return Ok(()); }
    let target = target(owner, pane.width, pane.height)?;
    let (backing, pitch, first) = wait_for(None, true, |c| {
        if !display::owner(owner) { return Some(Err(abi::errno::EPERM)); }
        let Some(r) = c.resources.get(&target.id) else { return Some(Err(EIO)); };
        if r.lost || r.dropped { return Some(Err(EIO)); }
        if r.writers != 0 || r.reading || display::write_blocked(c, &[target.id]) { return None; }
        let data = (r.backing.clone(), r.pitch as usize, r.last.is_none() || r.invalid);
        c.resources.get_mut(&target.id)?.reading = true;
        Some(Ok(data))
    }).map_err(|_| EINTR)??;
    // Initialize the whole surface on its first frame, including outside the dirty rectangle.
    if first { rows = 0..pane.height; columns = 0..pane.width; }
    let written = (|| {
        copy_rows(&backing, pitch, pane, rows.clone(), columns.clone())?;
        backing.maintain(rows.start * pitch, rows.len() * pitch, false);
        let (serial, fence) = with_core(|c| (c.serial(), c.device_fence())).ok_or(ENODEV)?;
        let offset = rows.start * pitch + columns.start * 4;
        let request = dev::transfer_to_host(Tag::Sync(serial), target.ctx, fence, target.id,
            backing.clone(), columns.start as u32, rows.start as u32,
            columns.len() as u32, rows.len() as u32, offset as u64, pitch as u32).ok_or(EIO)?;
        let done = exec_req(serial, request, target.ctx, &[target.id], &[target.id])?;
        if !dev::ok(done.answer) || !done.matches_fence(fence) { return Err(EIO); }
        Ok(LastWrite { ctx: target.ctx, device_ctx: dev::context_id(target.ctx), fence,
            serial, answer: done.answer, kind: dev::upload_kind() })
    })();
    with_core(|c| if let Some(r) = c.resources.get_mut(&target.id) {
        r.invalid = written.is_err();
        if let Ok(last) = &written { r.last = Some(*last); r.generation += 1; }
    });
    release_other(Some(target.id));
    written?;
    let (_, status, _) = display::present_as(owner, target.context, target.handle, &|_| Ok(()))?;
    if status == fence_status::OK { Ok(()) } else { Err(EIO) }
}

/// Observe the completed binding through the normal complementary-poison readback.
pub fn pixel(x: usize, y: usize) -> Result<u32, i64> {
    let _path = GpuPath::enter();
    let owner = caller()?;
    let _presenting = crate::console::vt::Presentation::begin(true).ok_or(crate::syscall::errno::EBUSY as i64)?;
    let _transition = display::Transition::for_observation(owner)?;
    let (id, last, backing, pitch, fmt) = wait_for(None, true, |c| {
        if !display::owner(owner) { return Some(Err(abi::errno::EPERM)); }
        let Some(binding) = c.display.binding.filter(|_| c.display.known) else { return Some(Err(EIO)); };
        let Some(r) = c.resources.get_mut(&binding.id) else { return Some(Err(EIO)); };
        if r.lost || r.invalid { return Some(Err(EIO)); }
        if x >= r.width as usize || y >= r.height as usize { return Some(Err(EINVAL)); }
        if r.writers != 0 || r.reading { return None; }
        r.reading = true;
        Some(Ok((binding.id, r.last, r.backing.clone(), r.pitch, r.format)))
    }).map_err(|_| EINTR)??;
    let observed = (|| {
        let stage = Staging::new(owner, 4)?;
        let args = abi::Readback { x: x as u32, y: y as u32, width: 1, height: 1, ..abi::Readback::ZERO };
        let seen = observe(owner, id, last, &backing, pitch, fmt, args, &stage.pages, None,
            #[cfg(target_arch = "x86_64")] None)
            .map_err(|(errno, _)| errno)?;
        if seen.unwritten != 0 { return Err(EIO); }
        let mut pixel = [0; 4]; stage.pages.read(0, &mut pixel);
        Ok(u32::from(pixel[2]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[0]))
    })();
    release_other(Some(id));
    observed
}
