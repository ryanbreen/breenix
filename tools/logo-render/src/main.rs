use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::PathBuf;

use breenix_logo::{render, Frame, Milestone, PixelFormat, Surface, BACKGROUND, HEIGHT, WIDTH};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: logo-render OUTPUT_DIRECTORY [REVIEW_DIRECTORY]",
        )
    })?;
    let review = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "too many arguments").into());
    }
    fs::create_dir_all(&output)?;
    if let Some(path) = &review {
        fs::create_dir_all(path)?;
    }
    let frame = Frame {
        cursor_visible: true,
        completed: 14,
        total: 23,
        milestones: [
            Milestone::Done,
            Milestone::Done,
            Milestone::Done,
            Milestone::Running,
            Milestone::Pending,
            Milestone::Pending,
        ],
    };
    for k in [2u16, 3, 4] {
        let padding = 24 * u32::from(k);
        let width = WIDTH * u32::from(k) + padding * 2;
        let height = HEIGHT * u32::from(k) + padding * 2;
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&[BACKGROUND.0, BACKGROUND.1, BACKGROUND.2]);
        }
        let mut surface = Surface::new(
            &mut pixels,
            width as usize,
            height as usize,
            width as usize * 3,
            PixelFormat::Rgb,
        )
        .map_err(|e| io::Error::other(format!("surface: {e:?}")))?;
        render(&mut surface, (padding as i32, padding as i32), k, &frame)
            .map_err(|e| io::Error::other(format!("render: {e:?}")))?;
        let name = format!("breenix-logo-k{k}.png");
        let path = output.join(&name);
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(&path)?), width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixels)?;
        writer.finish()?;
        if let Some(directory) = &review {
            fs::copy(&path, directory.join(&name))?;
        }
        println!("Rendered {name}: {width}x{height}");
    }
    Ok(())
}
