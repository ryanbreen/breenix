#[allow(dead_code)]
#[path = "../userspace/programs/src/guskit/canvas.rs"]
mod canvas;
#[path = "../userspace/programs/src/guskit/files.rs"]
mod files;
use canvas::Canvas;

#[test]
fn bmp_and_png_round_trip_exact_rgb() {
    let mut canvas = Canvas::new(3, 2);
    let white = canvas.digest();
    canvas.pencil(&[(0, 0)], [1, 2, 3]);
    canvas.pencil(&[(2, 1)], [17, 42, 201]);
    assert_ne!(canvas.digest(), white);
    for name in ["drawing.bmp", "drawing.png"] {
        let bytes = files::encode(name, &canvas, 3, 2).unwrap();
        let (opened, message) = files::decode(name, &bytes, 3, 2).unwrap();
        assert_eq!(opened.pixels(), canvas.pixels());
        assert_eq!(opened.digest(), canvas.digest());
        assert_eq!(opened.digest(), libpixcheck::sha256(opened.pixels()));
        assert!(message.is_none());
    }
}

#[test]
fn crop_and_pad_top_left() {
    let mut source = Canvas::new(3, 2);
    source.pencil(&[(0, 0)], [1, 2, 3]);
    source.pencil(&[(1, 1)], [4, 5, 6]);
    let bytes = files::encode("x.png", &source, 3, 2).unwrap();
    let (crop, message) = files::decode("x.png", &bytes, 2, 1).unwrap();
    assert_eq!(crop.pixels(), &[1, 2, 3, 255, 255, 255]);
    assert_eq!(message.as_deref(), Some("cropped from 3x2"));
    let (pad, message) = files::decode("x.png", &bytes, 4, 3).unwrap();
    assert_eq!(pad.pixels(), &[1,2,3, 255,255,255, 255,255,255, 255,255,255, 255,255,255, 4,5,6, 255,255,255, 255,255,255, 255,255,255, 255,255,255, 255,255,255, 255,255,255]);
    assert!(message.is_none());
}

#[test]
fn malformed_and_unknown_formats_are_rejected() {
    for name in ["x.bmp", "x.png", "x.gif"] {
        assert!(files::decode(name, b"not an image", 2, 2).is_err());
    }
    assert!(files::encode("x.gif", &Canvas::new(2, 2), 2, 2).is_err());
}
