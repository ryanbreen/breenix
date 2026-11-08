//! Author the UI app's canonical scene and x86-metal pin on the host.
#[path = "../../../../userspace/programs/src/uitest_canonical.rs"]
mod scene;
#[path = "../png.rs"]
mod png;
#[path = "../sha256.rs"]
mod sha256;
use libgfx_ref::{color::Color, framebuf::{Format,FrameBuf},shapes};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&out).expect("create output directory");
    let mut frame = FrameBuf::new_filled(scene::WIDTH,scene::HEIGHT,Format::BGRX,Color::rgb(0,0,0)).unwrap();
    scene::draw(|x,y,w,h,[r,g,b]| shapes::fill_rect(&mut frame,x,y,w,h,Color::rgb(r,g,b)));
    let mut pixels = Vec::with_capacity(scene::WIDTH*scene::HEIGHT*4);
    for y in 0..scene::HEIGHT { for x in 0..scene::WIDTH {
        let c = frame.get_pixel(x,y); pixels.extend_from_slice(&[c.b,c.g,c.r,255]);
    }}
    let pin = sha256::hex(&sha256::digest(&pixels));
    let file = png::encode_rgb(&frame);
    std::fs::write(out.join("canonical-scene.png"),&file).expect("write scene PNG");
    std::fs::write(out.join("canonical-x86-metal.sha256"),format!("{pin}\n")).expect("write canonical pin");
    println!("canonical-scene.png {}x{} sha256={} pixel_sha256={pin}",scene::WIDTH,scene::HEIGHT,sha256::hex(&sha256::digest(&file)));
}
