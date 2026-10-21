//! Client-owned window targets on the GPU core. Tree pages remain window mappings.
//!
//! The window holds a resource reference independently of its public handle. GPU requests
//! retain the core's DMA holds; dropping a window revokes the handle, then drops its hold.

use super::*;
use alloc::sync::Weak;

pub fn backend_code() -> u32 { with_core(|c| c.caps.backend).unwrap_or(0) }

static CONTEXTS: crate::irq_safe_mutex::IrqSafeMutex<BTreeMap<u64, Weak<Context>>> =
    crate::irq_safe_mutex::IrqSafeMutex::new(BTreeMap::new());

struct Context {
    owner: u64,
    handle: u64,
    id: u32,
}

impl Context {
    fn get(owner: u64) -> Result<Arc<Self>, i64> {
        if let Some(context) = CONTEXTS.lock().get(&owner).and_then(Weak::upgrade) {
            return Ok(context);
        }
        let (handle, _) = create_context(owner)?;
        let id = with_core(|c| c.ctx_of(owner, handle)).flatten().ok_or(EIO)?;
        let made = Arc::new(Self { owner, handle, id });
        let mut contexts = CONTEXTS.lock();
        if let Some(context) = contexts.get(&owner).and_then(Weak::upgrade) {
            drop(contexts);
            return Ok(context);
        }
        contexts.insert(owner, Arc::downgrade(&made));
        Ok(made)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        destroy_context(self.owner, self.handle);
        let mut contexts = CONTEXTS.lock();
        if contexts.get(&self.owner).is_some_and(|entry| entry.strong_count() == 0) {
            contexts.remove(&self.owner);
        }
    }
}

/// A window buffer. Handles are process-local; `id` is only for the compositor's core path.
pub struct Target {
    context: Arc<Context>,
    id: u32,
    handle: u64,
    width: u32,
    height: u32,
    bytes: u64,
}

impl Target {
    pub fn new(owner: u64, width: u32, height: u32) -> Result<Arc<Self>, i32> {
        Self::make(owner, width, height).map_err(|e| e as i32)
    }

    fn make(owner: u64, width: u32, height: u32) -> Result<Arc<Self>, i64> {
        let context = Context::get(owner)?;
        let _creation = CreationGuard::acquire(owner)?;
        if !with_core(|c| c.proc_mut(owner).resources.len() < RESOURCE_QUOTA).unwrap_or(false) {
            return Err(EAGAIN);
        }
        let pitch = if dev::is_svga() { width * 4 } else { (width * 4).next_multiple_of(256) };
        let bytes = (pitch as usize * height as usize).div_ceil(PAGE) * PAGE;
        let pages = device_frames(owner, bytes)?;
        let id = console::resource_with(context.id, kind::TARGET, format::B8G8R8A8,
            width, height, false, pages)?;
        let handle = with_core(|c| {
            let handle = c.handle(owner);
            let resource = c.resources.get_mut(&id).ok_or(EIO)?;
            resource.owner = owner;
            c.proc_mut(owner).resources.insert(handle, id);
            Ok::<u64,i64>(handle)
        }).ok_or(ENODEV)??;
        let target = Arc::new(Self { context, id, handle, width, height, bytes: bytes as u64 });
        let contexts = with_core(|c| c.procs.get(&owner).map(|p| p.contexts.values().copied().collect::<Vec<_>>()))
            .flatten().ok_or(EIO)?;
        let pairs: Vec<_> = contexts.into_iter().map(|ctx| (ctx, id)).collect();
        for attached in attach_all(&pairs) { attached?; }
        target.clear()?;
        Ok(target)
    }

    pub fn revoke(&self) { let _ = destroy_resource(self.context.owner,self.handle); }

    pub fn handle(&self) -> u64 { self.handle }
    pub fn device_id(&self) -> u32 { self.id }
    pub fn len(&self) -> u64 { self.bytes }

    /// Serialize sealing with SUBMIT admission. Already admitted writes keep their holds.
    pub fn seal(&self, sealed: bool) -> Result<(), i32> {
        with_core(|c| {
            let r = c.resources.get_mut(&self.id).ok_or(EIO)?;
            if r.lost || (sealed && r.invalid && r.writers == 0) { return Err(EIO); }
            r.window_sealed = sealed;
            Ok(())
        }).unwrap_or(Err(ENODEV)).map_err(|e| e as i32)
    }

    /// Wait for accepted renders before sampling, replay or capture; invalid contents return EIO.
    pub fn ready(&self) -> Result<(), i32> {
        wait_for(None, true, |c| {
            let Some(r) = c.resources.get(&self.id) else { return Some(Err(EIO)); };
            if r.lost { return Some(Err(EIO)); }
            if r.writers != 0 || r.reading { return None; }
            Some(if r.invalid { Err(EIO) } else { Ok(()) })
        }).map_err(|_| EINTR).and_then(|r| r).map_err(|e| e as i32)
    }

    pub fn clear(&self) -> Result<(), i64> {
        let mut words = Vec::new();
        stream::encode::begin(&mut words, self.id as u64);
        stream::encode::clear(&mut words, 0xff000000);
        self.execute(&words, None)
    }

    pub fn copy_from(&self, source: &Target, rects: &[breengel_abi::Rect]) -> Result<(), i32> {
        source.ready()?;
        let mut words = Vec::new();
        for r in rects.iter().filter(|r| !r.is_empty()) {
            if r.x < 0 || r.y < 0 || r.x as u32 + r.w as u32 > self.width.min(source.width)
                || r.y as u32 + r.h as u32 > self.height.min(source.height) { return Err(EINVAL as i32); }
            stream::encode::copy(&mut words, source.id as u64,
                [r.x as u32, r.y as u32, r.w as u32, r.h as u32], self.id as u64, [r.x as u32,r.y as u32]);
        }
        if words.is_empty() { return Ok(()); }
        self.execute(&words, Some(source)).map_err(|e| e as i32)
    }

    fn execute(&self, words: &[u32], source: Option<&Target>) -> Result<(), i64> {
        let ctx = self.context.id;
        if let Some(src) = source {
            exec_ok(|tag| Some(dev::ctx_attach(tag, ctx, src.id)), ctx, &[src.id])?;
        }
        wait_for(None, true, |c| {
            let context = c.contexts.get(&ctx)?;
            if context.state != CtxState::Live || context.dropped { return Some(Err(EIO)); }
            let r = c.resources.get(&self.id)?;
            if context.busy || !context.inflight.is_empty() || r.writers != 0 || r.reading { return None; }
            c.contexts.get_mut(&ctx)?.busy = true;
            c.resources.get_mut(&self.id)?.writers += 1;
            Some(Ok(()))
        }).map_err(|_| EINTR)??;
        let result = self.execute_taken(words, source);
        with_core(|c| {
            if let Some(context) = c.contexts.get_mut(&ctx) { context.busy = false; }
            if let Some(r) = c.resources.get_mut(&self.id) {
                r.writers = r.writers.saturating_sub(1);
                if result.is_err() { r.invalid = true; }
            }
        });
        notify();
        result
    }

    fn execute_taken(&self, words: &[u32], source: Option<&Target>) -> Result<(), i64> {
        let ctx = self.context.id;
        let bindings = Bindings { destination: self, source };
        let (mut objects, mut dx, vb, scratch, scratch_texture) = with_core(|c| c.contexts.get(&ctx)
            .map(|x| (x.objects.clone(),x.dx.clone(),x.vb,x.scratch,x.scratch_texture))).flatten().ok_or(EIO)?;
        let chunks = if dev::is_svga() {
            let extras = || svga::encode::Extras { scratch: [[self.width,self.height],[0,0]], data_width: 1,
                corrupt: None, suppress: None, swap: None };
            let (sized,_) = svga::encode::translate(&mut dx.clone(), &[], words, &bindings, extras(), None, None).ok_or(ENOMEM)?;
            let backing = svga::encode::Backing::allocate(&sized.sizes).ok_or(ENOMEM)?;
            let (out,_) = svga::encode::translate(&mut dx, &[], words, &bindings, extras(), Some(backing), None).ok_or(ENOMEM)?;
            Chunks::Svga(out.chunks)
        } else {
            let mut enc = encode::Enc::new();
            encode::translate(&mut enc,&mut objects,words,&bindings,encode::Extras {
                vertex_buffer: vb, scratch: [scratch,scratch_texture], data:0,data_width:0,flip_y:FLIP_Y,
                corrupt:None,suppress:None,swap:None });
            Chunks::Virgl(enc.finish().ok_or(ENOMEM)?)
        };
        with_core(|c| if let Some(x) = c.contexts.get_mut(&ctx) { x.objects = objects; x.dx = dx; });
        let mut ids = alloc::vec![self.id,vb,scratch,scratch_texture];
        if let Some(src) = source { ids.push(src.id); }
        let mut last = None;
        let mut send = |request: Request, serial: u64, fence: u64| -> Result<(), i64> {
            let answer = exec_req(serial,request,ctx,&ids,&[self.id])?;
            if !dev::ok(answer.answer) || !answer.fenced || answer.echoed != fence { return Err(EIO); }
            last = Some(LastWrite { ctx,device_ctx:dev::context_id(ctx),fence,serial,answer:answer.answer,kind:dev::draw_kind() });
            Ok(())
        };
        match chunks {
            Chunks::Virgl(chunks) => for chunk in chunks {
                let (serial,fence) = with_core(|c| (c.serial(),c.device_fence())).ok_or(ENODEV)?;
                send(dev::submit_virgl(Tag::Sync(serial),ctx,Some(fence),chunk),serial,fence)?;
            },
            Chunks::Svga(chunks) => for chunk in chunks {
                let (serial,fence) = with_core(|c| (c.serial(),c.device_fence())).ok_or(ENODEV)?;
                send(dev::submit_svga(Tag::Sync(serial),ctx,Some(fence),chunk).ok_or(EIO)?,serial,fence)?;
            },
        }
        with_core(|c| if let Some(r) = c.resources.get_mut(&self.id) {
            r.last = last; r.generation += 1; r.invalid = false;
        });
        Ok(())
    }

    /// Device observation of a pinned target, independent of the public handle's lifetime.
    pub fn read_to(&self, dst: u64, pause_at: Option<u64>, pause: impl Fn()) -> Result<(), i32> {
        self.read_to_inner(dst,pause_at,pause).map_err(|e| e as i32)
    }

    fn read_to_inner(&self, dst: u64, pause_at: Option<u64>, pause: impl Fn()) -> Result<(), i64> {
        let owner = caller()?;
        let context = Context::get(owner)?;
        exec_ok(|t| Some(dev::ctx_attach(t,context.id,self.id)),context.id,&[self.id])?;
        let (last,backing,pitch) = wait_for(None,true,|c| {
            let Some(r) = c.resources.get_mut(&self.id) else { return Some(Err(EIO)); };
            if r.lost { return Some(Err(EIO)); }
            if r.writers != 0 || r.reading { return None; }
            if r.invalid { return Some(Err(EIO)); }
            r.reading = true;
            Some(Ok((r.last,r.backing.clone(),r.pitch)))
        }).map_err(|_| EINTR)??;
        let result = (|| {
            let bytes = self.width as usize * self.height as usize * 4;
            let stage = Staging::new(owner,bytes)?;
            let args = abi::Readback { target:self.handle,width:self.width,height:self.height,..abi::Readback::ZERO };
            let seen = observe(owner,self.id,last,&backing,pitch,format::B8G8R8A8,args,&stage.pages)
                .map_err(|(errno,_)| errno)?;
            if seen.unwritten != 0 { return Err(EIO); }
            let mut row = [0u8;512];
            let stride = self.width as usize * 4;
            for y in 0..self.height as usize {
                if pause_at == Some(y as u64) { pause(); }
                for at in (0..stride).step_by(row.len()) {
                    let n = row.len().min(stride-at);
                    stage.pages.read(y*stride+at,&mut row[..n]);
                    for pixel in row[..n].chunks_exact_mut(4) { pixel[3] = 255; }
                    crate::syscall::userptr::write_user_bytes(dst+(y*stride+at) as u64,row.as_ptr(),n).map_err(|_| EFAULT)?;
                }
            }
            Ok(())
        })();
        release_other(Some(self.id));
        result
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        let _ = destroy_resource(self.context.owner,self.handle);
        with_core(|c| c.unhold(self.id));
        notify();
    }
}

struct Bindings<'a> { destination: &'a Target, source: Option<&'a Target> }
impl Bindings<'_> {
    fn find(&self, handle: u64) -> Option<&Target> {
        if handle == self.destination.id as u64 { Some(self.destination) }
        else { self.source.filter(|s| s.id as u64 == handle) }
    }
}
impl encode::Bindings for Bindings<'_> {
    fn target(&self, handle: u64) -> Option<encode::TargetRes> {
        let t = self.find(handle)?;
        Some(encode::TargetRes { id:t.id,format:wire::format::B8G8R8A8_UNORM,
            info:TargetInfo { width:t.width,height:t.height,invalid:false,format:format::B8G8R8A8 } })
    }
    fn source(&self, handle: u64) -> Option<encode::SourceRes> {
        let t = self.find(handle)?;
        Some(encode::SourceRes { id:t.id,format:wire::format::B8G8R8A8_UNORM,width:t.width,height:t.height })
    }
    fn buffer_rect(&self, _handle:u64, _offset:u64) -> [i32;4] { [0;4] }
    fn buffer_word(&self, _handle:u64, _offset:u64) -> u32 { 0 }
}
