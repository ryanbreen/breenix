//! Generate pinned app-scenario references with the in-tree widget/CPU renderers.
use gfx_refgen::widget_references;
fn main() {
    let mut args = std::env::args().skip(1);
    let scenario = args.next().expect("scenario name");
    let out = std::path::PathBuf::from(args.next().expect("output directory"));
    assert!(args.next().is_none(), "unexpected argument");
    std::fs::create_dir_all(&out).unwrap();
    let stages: &[&str] = match scenario.as_str() {
        "form" => &["initial", "typed", "checked", "slider", "submitted"],
        "menu" => &["initial", "opened", "chosen"],
        "list" => &["initial", "scrolled", "selected"],
        _ => panic!("unknown scenario {scenario}"),
    };
    for scale in [1, 2] {
        for (index, stage) in stages.iter().enumerate() {
            let frame = match scenario.as_str() {
                "form" => widget_references::form(index, scale),
                "menu" => widget_references::menu(index, scale),
                "list" => widget_references::list(index, scale),
                _ => unreachable!(),
            };
            let mut pixels = Vec::with_capacity(frame.width * frame.height * 4);
            for y in 0..frame.height {
                for x in 0..frame.width {
                    let c = frame.get_pixel(x, y);
                    pixels.extend_from_slice(&[c.b, c.g, c.r, 255]);
                }
            }
            let file = libimg_encode::encode(&libimg::png::Image {
                width: frame.width as u32,
                height: frame.height as u32,
                pixels,
            })
            .unwrap();
            let name = format!("{scenario}-{stage}-s{scale}.png");
            std::fs::write(out.join(&name), file).unwrap();
            println!("{name}");
        }
    }
}
