//! Small application used by UI session tests.
mod uitest_app_support;
mod uitest_canonical;

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use breengel::{Event, Window};

fn main() {
    libuitest::startup::mark("app-exec");
    let args: Vec<_> = std::env::args_os().collect();
    let env: Vec<Vec<u8>> = std::env::vars_os().map(|(key, value)| {
        let mut entry = key.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        entry
    }).collect();
    let arg_bytes: Vec<&[u8]> = args.iter().map(|s| s.as_bytes()).collect();
    let env_bytes: Vec<&[u8]> = env.iter().map(Vec::as_slice).collect();
    println!("{}", uitest_app_support::echo(&arg_bytes, &env_bytes));
    let _ = io::stdout().flush();
    let strings: Vec<&str> = match args.iter().skip(1).map(|s| s.to_str().ok_or("non-UTF-8 argument")).collect() {
        Ok(strings) => strings,
        Err(error) => { eprintln!("{error}"); std::process::exit(2); }
    };
    let options = match uitest_app_support::parse(&strings) {
        Ok(options) => options,
        Err(error) => { eprintln!("{error}"); std::process::exit(2); }
    };
    let mut win = Window::new_without_fonts(options.title.as_bytes(), 320, 200)
        .unwrap_or_else(|error| { eprintln!("window: {error}"); std::process::exit(1); });
    if options.canonical {
        draw_canonical(&mut win);
    } else {
        let generation = libuitest::capture_marker::Generation { commit: 1,
            producer: u32::try_from(libbreenix::process::gettid().expect("producer tid").raw()).expect("producer tid fits marker") };
        let marker = libuitest::capture_marker::encode(generation);
        let surface = win.surface().unwrap_or_else(|error| { eprintln!("surface: {error}"); std::process::exit(1); });
        surface.batch(|surface| {
            surface.clear(libgfx::color::Color::rgb(0x18,0x20,0x30));
            for (x, pixel) in marker.chunks_exact(4).enumerate() {
                libgfx::shapes::fill_rect(surface, x as i32, 0, 1, 200, libgfx::color::Color::rgb(pixel[2],pixel[1],pixel[0]));
            }
        });
        win.write_tree(&libuitest::capture::root(generation, 320, 200)).expect("generation tree");
        let committed = win.commit(&[libbreenix::window::abi::Rect::new(0,0,320,200)]).expect("generation commit");
        assert_eq!(committed.commit_id, generation.commit, "producer ordinal differs from commit id");
    }
    libuitest::startup::mark("app-commit");
    if options.session_probe {
        extern "C" {
            fn getuid() -> u32;
            fn geteuid() -> u32;
            fn access(path: *const u8, mode: i32) -> i32;
            fn setlocale(category: i32, name: *const u8) -> *const std::ffi::c_char;
        }
        let root = std::env::var("BREENIX_CONFIG_ROOT").expect("session root");
        let root = std::path::Path::new(&root);
        let font_config = std::fs::read(root.join("etc/fonts.conf")).expect("session fonts");
        let locale = std::fs::read(root.join("etc/locale.conf")).expect("session locale");
        let config_path = std::ffi::CString::new(root.join("etc/fonts.conf").as_os_str().as_bytes()).unwrap();
        // Breenix's <locale.h> assigns LC_ALL=0. An empty name selects LANG.
        let selected = unsafe { setlocale(0, b"\0".as_ptr()) };
        assert!(!selected.is_null(), "session locale refused");
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(selected) }.to_bytes(), b"C.UTF-8");
        println!("UITEST-SESSION uid={} euid={} clock={} epoch={} seed={} mode={} fonts={} locale={} config_writable={}",
            unsafe { getuid() }, unsafe { geteuid() }, libbreenix::ui_clock::now_ns(), libbreenix::ui_clock::epoch(),
            std::env::var("BREENIX_UI_SEED").unwrap(), std::env::var("BREENIX_UI_MODE").unwrap(),
            libuitest::fixtures::digest(&font_config), libuitest::fixtures::digest(&locale),
            unsafe { access(config_path.as_ptr().cast(), 2) } == 0);
        let _ = io::stdout().flush();
    }
    if options.config_probe {
        let root = std::env::var_os("BREENIX_CONFIG_ROOT").map(std::path::PathBuf::from).unwrap_or_else(|| "/".into());
        let fonts = breengel::FontConfig::try_load().expect("rooted font config");
        let theme = std::fs::read_to_string(root.join("etc/app.conf")).expect("rooted app config");
        std::fs::write(root.join("var/log/uitest-app"), &theme).expect("rooted app log");
        println!("UITEST-CONFIG {fonts:?} {}", theme.trim_end());
        let _ = io::stdout().flush();
    }
    let block = [b'x'; 4096];
    let mut remaining = options.stderr_flood;
    let mut stderr = io::stderr().lock();
    while remaining != 0 {
        let count = remaining.min(block.len());
        if stderr.write_all(&block[..count]).is_err() { break; }
        remaining -= count;
    }
    drop(stderr);
    if let Some(signal) = options.crash {
        let _ = libbreenix::signal::kill(libbreenix::process::getpid().expect("getpid").raw() as i32, signal);
    }
    if options.hang { loop { let _ = libbreenix::time::sleep_ms(1000); } }
    loop {
        let events = win.wait_event();
        if events.iter().any(|event| matches!(event, Event::CloseRequested)) { break; }
        if options.canonical {
            for event in events {
                if let Event::MouseMove { x,y } = event {
                    let commit = draw_canonical(&mut win);
                    println!("UITEST-CURSOR x={x} y={y} commit={commit}");
                    let _ = io::stdout().flush();
                }
            }
        }
    }
}

fn draw_canonical(win: &mut Window) -> u64 {
    let surface = win.surface().expect("canonical surface");
    surface.batch(|surface| {
        uitest_canonical::draw(|x,y,w,h,[r,g,b]| {
            libgfx::shapes::fill_rect(surface,x,y,w,h,libgfx::color::Color::rgb(r,g,b));
        });
    });
    win.write_tree(b"{\"key\":\"root\",\"parent\":null,\"role\":\"window\",\"name\":\"canonical-v1\",\"value\":\"static\",\"bounds\":[0,0,320,200],\"clip\":[0,0,320,200],\"flags\":[\"enabled\",\"visible\"]}\0").expect("canonical tree");
    win.commit(&[libbreenix::window::abi::Rect::new(0,0,uitest_canonical::WIDTH as i32,uitest_canonical::HEIGHT as i32)])
        .expect("canonical commit").commit_id
}
