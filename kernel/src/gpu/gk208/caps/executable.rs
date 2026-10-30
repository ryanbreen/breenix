//! Pre-install execution on the sole native startup owner's live device.
use super::{ProbeResults, PROBE_BYTES as BYTES, PROBE_HEIGHT as H, PROBE_WIDTH as W};
use crate::gk208::{adapter, bulk, encode, hash, hex};
use crate::gpu::{
    dev::{self, Reap, Request, Tag},
    dma::Pages,
};
use alloc::{format, sync::Arc, vec, vec::Vec};
use gpu_abi::{
    capability as c, format as f, kind, pipeline,
    stream::{self, Resolver, SourceInfo, TargetInfo},
};

const BGRA: u32 = 0xffff_0001;
const RGBA: u32 = 0xffff_0002;
const R8: u32 = 0xffff_0003;
const OUTPUT: u32 = 0xffff_0004;

struct Image {
    id: u32,
    format: u32,
    backing: Arc<Pages>,
}
struct Runner<'a> {
    device: &'a mut adapter::Device,
    caller: u64,
}
impl Runner<'_> {
    fn execute(&mut self, request: Option<Request>) -> Result<(), &'static str> {
        let request = request.ok_or("native CAPS builder refused")?;
        let fence = request.fence();
        let Request::Gk208(request) = request else {
            return Err("native CAPS builder selected another backend");
        };
        if self.device.send(request).is_err() {
            return Err("native CAPS submission refused");
        }
        let start = crate::arch_impl::x86_64::timer::rdtsc();
        loop {
            match self.device.reap() {
                Reap::Done(done) => {
                    if done.answer != gpu_abi::gk208::SUCCESS
                        || fence.is_some_and(|fence| !done.matches_fence(fence))
                        || fence.is_none() && done.fenced
                    {
                        return Err("native CAPS completion mismatch");
                    }
                    return Ok(());
                }
                Reap::Fault => return Err(self.device.fault_reason().unwrap_or("native CAPS engine fault")),
                Reap::Empty => (),
            }
            if crate::gk208::channel::elapsed(start) > 2_000_000 {
                return Err("native CAPS completion timed out");
            }
            // Wait with interrupts enabled; no core or allocator lock is held.
            core::hint::spin_loop();
        }
    }
    fn next(&mut self) -> (Tag, u64) {
        self.caller += 1;
        (Tag::Sync(self.caller), self.caller)
    }
    fn image(&mut self, id: u32, format: u32) -> Result<Image, &'static str> {
        let (tag, _) = self.next();
        self.execute(dev::resource_create(
            tag,
            id,
            if format == f::R8 {
                kind::TEXTURE
            } else {
                kind::TARGET
            },
            format,
            W,
            H,
            false,
        ))?;
        let pitch = (W * f::bytes(format)).next_multiple_of(64);
        let backing = Arc::new(
            Pages::new((pitch as usize * H as usize).div_ceil(4096))
                .ok_or("native CAPS backing exhausted")?,
        );
        let (tag, _) = self.next();
        self.execute(dev::attach_backing(tag, id, backing.clone()))?;
        Ok(Image {
            id,
            format,
            backing,
        })
    }
    fn upload(&mut self, image: &Image, pixels: &[u8]) -> Result<(), &'static str> {
        let row = (W * f::bytes(image.format)) as usize;
        let stride = row.next_multiple_of(64);
        if pixels.len() != row * H as usize {
            return Err("native CAPS input extent");
        }
        for (y, line) in pixels.chunks_exact(row).enumerate() {
            image.backing.write(y * stride, line);
        }
        image.backing.maintain(0, stride * H as usize, false);
        let (tag, fence) = self.next();
        self.execute(dev::transfer_to_host(
            tag,
            0,
            fence,
            image.id,
            image.backing.clone(),
            0,
            0,
            W,
            H,
            0,
            stride as u32,
        ))
    }
    fn read(&mut self, image: &Image) -> Result<[Vec<u8>; 2], &'static str> {
        let mut images = [vec![0; BYTES], vec![0; BYTES]];
        for (poison, bytes) in [0x55, 0xaa].into_iter().zip(&mut images) {
            image.backing.write(0, &vec![poison; BYTES]);
            image.backing.maintain(0, BYTES, false);
            let (tag, fence) = self.next();
            self.execute(dev::transfer_from_host(
                tag,
                0,
                fence,
                image.id,
                image.backing.clone(),
                0,
                0,
                W,
                H,
                0,
                W * 4,
            ))?;
            image.backing.maintain(0, BYTES, true);
            image.backing.read(0, bytes);
        }
        Ok(images)
    }
    fn draw(&mut self, mut frame: encode::Frame) -> Result<(), &'static str> {
        let pages = Arc::new(
            Pages::new(frame.data.len().div_ceil(4096))
                .ok_or("native CAPS draw backing exhausted")?,
        );
        pages.write(0, &frame.data);
        pages.maintain(0, frame.data.len(), false);
        // Leave room for the publisher's parking state and release in its
        // 2048-word slot; batching retains the original draw packets.
        frame.batch_draws(1400);
        for chunk in encode::prepare(frame, pages)? {
            let (tag, fence) = self.next();
            self.execute(Some(dev::submit_gk208(tag, fence, chunk)))?;
        }
        Ok(())
    }
    fn render(&mut self, image: &Image) -> Result<(), &'static str> {
        self.upload(image, &vec![0x3d; BYTES])?;
        let view = View::new(image, None)?;
        // One solid draw per pixel supplies its unique colour input.
        let mut words = Vec::new();
        stream::encode::begin(&mut words, u64::from(image.id));
        for (n, pixel) in include_bytes!("colour.bgra").chunks_exact(4).enumerate() {
            let (x, y) = (n as i32 % W as i32, n as i32 / W as i32);
            stream::encode::draw(
                &mut words,
                pipeline::SOLID,
                u32::from_le_bytes(pixel.try_into().unwrap()),
                &[[x, y, x + 1, y + 1]],
            );
        }
        self.draw(encode::translate(words.as_slice(), &view)?)
    }
    fn sample(&mut self, source: &Image, destination: &Image) -> Result<(), &'static str> {
        self.upload(destination, &vec![0x3d; BYTES])?;
        let view = View::new(destination, Some(source))?;
        let mut words = Vec::new();
        stream::encode::begin(&mut words, u64::from(destination.id));
        stream::encode::draw_sampled(
            &mut words,
            pipeline::COPY,
            u64::from(source.id),
            &[[0, 0, W as i32, H as i32, 0, 0]],
        );
        self.draw(encode::translate(words.as_slice(), &view)?)
    }
}
struct View {
    target: encode::Target,
    source: Option<encode::Target>,
}
impl View {
    fn new(target: &Image, source: Option<&Image>) -> Result<Self, &'static str> {
        Ok(Self {
            target: encode::target(target.id, target.format).ok_or("native CAPS target absent")?,
            source: source
                .map(|s| encode::target(s.id, s.format).ok_or("native CAPS source absent"))
                .transpose()?,
        })
    }
}
impl Resolver for View {
    fn target(&self, handle: u64) -> Option<TargetInfo> {
        (handle == u64::from(self.target.id)).then_some(TargetInfo {
            width: W,
            height: H,
            format: self.target.format,
            invalid: false,
        })
    }
    fn source(&self, handle: u64) -> Option<SourceInfo> {
        self.source
            .filter(|s| u64::from(s.id) == handle)
            .map(|s| SourceInfo {
                width: W,
                height: H,
                format: s.format,
                invalid: false,
            })
    }
    fn buffer_size(&self, _: u64) -> Option<u64> {
        None
    }
    fn buffer_rect(&self, _: u64, _: u64) -> [i32; 4] {
        [0; 4]
    }
}
impl encode::Bindings for View {
    fn render_target(&self, handle: u64) -> Option<encode::Target> {
        self.target(handle).map(|_| self.target)
    }
    fn sampled_source(&self, handle: u64) -> Option<encode::Target> {
        self.source.filter(|s| u64::from(s.id) == handle)
    }
}

pub(in crate::gk208) fn run(device: &mut adapter::Device) -> Result<(), &'static str> {
    run_epoch(device, None)
}
pub(in crate::gk208) fn run_reprobe(device: &mut adapter::Device, epoch: u64) -> Result<(), &'static str> {
    run_epoch(device, Some(epoch))
}
fn run_epoch(device: &mut adapter::Device, epoch: Option<u64>) -> Result<(), &'static str> {
    let start = crate::arch_impl::x86_64::timer::rdtsc();
    let mut r = Runner {
        device,
        caller: 1 << 48,
    };
    let mut probes = ProbeResults::new();
    let bgra = r
        .image(BGRA, f::B8G8R8A8)
        .map_err(|_| c::name(c::BGRA_RENDER))?;
    let rgba = r
        .image(RGBA, f::R8G8B8A8)
        .map_err(|_| c::name(c::RGBA_RENDER))?;
    let r8 = r.image(R8, f::R8).map_err(|_| c::name(c::R8_SAMPLE))?;
    let output = r
        .image(OUTPUT, f::B8G8R8A8)
        .map_err(|_| c::name(c::BGRA_SAMPLE))?;
    for id in c::BGRA_RENDER..=c::REGION_COPY {
        let result = (|| match id {
            c::BGRA_RENDER => {
                r.render(&bgra)?;
                r.read(&bgra)
            }
            c::BGRA_SAMPLE => {
                r.sample(&bgra, &output)?;
                r.read(&output)
            }
            c::RGBA_RENDER => {
                r.render(&rgba)?;
                r.read(&rgba)
            }
            c::RGBA_SAMPLE => {
                r.sample(&rgba, &output)?;
                r.read(&output)
            }
            c::R8_SAMPLE => {
                let mut both = [Vec::new(), Vec::new()];
                for plane in include_bytes!("r8.inputs").chunks_exact((W * H) as usize) {
                    r.upload(&r8, plane)?;
                    r.sample(&r8, &output)?;
                    for (all, image) in both.iter_mut().zip(r.read(&output)?) {
                        all.extend(image);
                    }
                }
                Ok(both)
            }
            c::INTEGER_OPS => {
                r.upload(&output, &vec![0x3d; BYTES])?;
                let target = encode::target(OUTPUT, f::B8G8R8A8)
                    .ok_or("native CAPS integer target absent")?;
                r.draw(encode::probe_frame(target, None))?;
                r.read(&output)
            }
            c::TEXEL_FETCH => {
                r.upload(&output, &vec![0x3d; BYTES])?;
                let view = View::new(&output, Some(&bgra))?;
                r.draw(encode::probe_frame(view.target, view.source))?;
                r.read(&output)
            }
            c::REGION_COPY => {
                let (tag, fence) = r.next();
                r.execute(dev::copy_region(
                    tag,
                    fence,
                    BGRA,
                    BGRA,
                    [3, 2, 73, 13, 2, 1],
                ))?;
                r.read(&bgra)
            }
            _ => unreachable!(),
        })();
        let (completed, first, second, error) = match result {
            Ok([a, b]) => (true, a, b, ""),
            Err(error) => (false, Vec::new(), Vec::new(), error),
        };
        probes.record(id, completed, &first, &second);
        r.device.set_probe_results(probes);
        let expected = super::probe_expected(id).unwrap();
        let phase = if epoch.is_some() { "caps-reprobe" } else { "caps-probe" };
        let incarnation = epoch.map(|e| format!(",\"epoch\":{e}")).unwrap_or_default();
        // Startup uploads the complete paired images. A fresh run hashes its
        // actual readbacks; only exact images can reference those startup bytes.
        // Failed or inexact observations retain their own bytes for diagnosis.
        let images = if epoch.is_some() && first == expected && second == expected {
            format!("\"first_sha256\":\"{}\",\"second_sha256\":\"{}\"", hash(&first), hash(&second))
        } else {
            format!("\"first\":\"{}\",\"second\":\"{}\"", hex(&first), hex(&second))
        };
        let row = format!(concat!("{{\"schema\":1,\"phase\":\"{}\"{},\"capability\":{},\"width\":{},\"height\":{},",
            "\"completed\":{},\"exact\":{},\"error\":\"{}\",\"expected_sha256\":\"{}\",{}}}"),
            phase, incarnation, id, W, H, completed,
            completed && first == expected && second == expected, error, hash(expected), images);
        let digest = hash(row.as_bytes());
        bulk::line(row)?;
        if let Some(epoch) = epoch {
            crate::serial_println!("GT710_CAPS_REPROBE id={} epoch={} sha256={}", id, epoch, digest);
        } else {
            crate::serial_println!("GT710_CAPS_PROBE id={} sha256={}", id, digest);
        }
        if r.device.outstanding() != 0 || r.device.needs_reset() {
            return Err(c::name(id));
        }
    }
    // Native device is drained: unref may now collect the probe mappings.
    for image in [bgra, rgba, r8, output] {
        let (tag, _) = r.next();
        r.execute(dev::resource_unref(tag, image.id, Some(image.backing)))?;
    }
    let elapsed = crate::gk208::channel::elapsed(start);
    if let Some(epoch) = epoch {
        crate::serial_println!("GT710_CAPS_REPROBE_TIME epoch={} us={}", epoch, elapsed);
    } else {
        crate::serial_println!("GT710_CAPS_PROBE_TIME us={}", elapsed);
    }
    Ok(())
}
