//! Image conversion for Gus Kit's RGB canvas.
use super::canvas::Canvas;

fn format(name: &str) -> Result<&str, String> {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "bmp" => Ok("bmp"),
        "png" => Ok("png"),
        _ => Err("unsupported image extension".into()),
    }
}

pub fn encode(name: &str, canvas: &Canvas, width: usize, height: usize) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    match format(name)? {
        "bmp" => Ok(libimg::bmp::encode_bmp_24(w, h, canvas.pixels())),
        _ => {
            let pixels = canvas.pixels().chunks_exact(3).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
            libimg_encode::encode(&libimg::png::Image { width: w, height: h, pixels }).map_err(|e| format!("PNG encode: {e:?}"))
        }
    }
}

/// Passing 0 for width or height selects the native image dimension.
pub fn decode(name: &str, bytes: &[u8], width: usize, height: usize) -> Result<(Canvas, Option<String>), String> {
    // Inspect dimensions before the decoder allocates its native-size buffers.
    // Four megapixels keeps an imported RGB state within 12 MiB. History has
    // its own byte budget; a phone photo is refused without replacing the drawing.
    let native = match format(name)? {
        "bmp" if bytes.len() >= 26 && &bytes[..2] == b"BM" => {
            if u32::from_le_bytes(bytes[14..18].try_into().unwrap()) == 12 {
                Some((u64::from(u16::from_le_bytes(bytes[18..20].try_into().unwrap())),
                      u64::from(u16::from_le_bytes(bytes[20..22].try_into().unwrap()))))
            } else {
                let w = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
                let h = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
                Some((u64::from(w), u64::from(h.unsigned_abs())))
            }
        }
        "png" if bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR" =>
            Some((u64::from(u32::from_be_bytes(bytes[16..20].try_into().unwrap())),
                  u64::from(u32::from_be_bytes(bytes[20..24].try_into().unwrap())))),
        _ => None,
    };
    if let Some((w,h)) = native {
        if w > 4096 || h > 4096 || w * h > 4 * 1024 * 1024 {
            return Err(format!("Image {w}x{h} exceeds document memory limit (4096 per side, 4 megapixels); drawing unchanged"));
        }
    }
    let (w, h, rgb) = match format(name)? {
        "bmp" => { let image = libimg::bmp::decode_bmp(bytes).map_err(|e| format!("BMP decode: {e:?}"))?; (image.width, image.height, image.rgb) },
        _ => {
            let image = libimg::png::decode(bytes).map_err(|e| format!("PNG decode: {e:?}"))?;
            // Decoded PNG is premultiplied BGRA; composite transparent pixels on white.
            let rgb = image.pixels.chunks_exact(4).flat_map(|p| [p[2].saturating_add(255-p[3]), p[1].saturating_add(255-p[3]), p[0].saturating_add(255-p[3])]).collect();
            (image.width, image.height, rgb)
        }
    };
    let width = if width == 0 { w as usize } else { width };
    let height = if height == 0 { h as usize } else { height };
    let mut canvas = Canvas::new(width, height);
    let copy_width = width.min(w as usize) * 3;
    for y in 0..height.min(h as usize) {
        canvas.pixels_mut()[y * width * 3..y * width * 3 + copy_width].copy_from_slice(&rgb[y * w as usize * 3..y * w as usize * 3 + copy_width]);
    }
    let message = (w as usize > width || h as usize > height).then(|| format!("cropped from {w}x{h}"));
    Ok((canvas, message))
}
