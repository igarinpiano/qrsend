//! Shows the stream in a native window (largest, densest option).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use minifb::{Key, KeyRepeat, ScaleMode, Window, WindowOptions};

use super::{FrameStream, GridSpec, layout_grid};

const WHITE: u32 = 0x00FF_FFFF;
const BLACK: u32 = 0x0000_0000;

/// `min_scale` is the smallest module size (pixels) an automatic grid may use.
pub fn run(stream: &mut FrameStream, mut fps: f64, grid: GridSpec, min_scale: usize) -> Result<()> {
    let opts = WindowOptions {
        resize: true,
        scale_mode: ScaleMode::UpperLeft,
        ..WindowOptions::default()
    };
    let mut window = Window::new("QRSend", 900, 900, opts)
        .context("cannot open a window (try --display terminal)")?;
    #[cfg(target_os = "macos")]
    dock_icon();
    window.set_target_fps(120);

    let modules = stream.params.modules();
    let mut buf: Vec<u32> = Vec::new();
    let mut paused = false;
    let mut next = Instant::now();
    let mut last_title = Instant::now() - Duration::from_secs(2);
    let mut dirty = true;
    let mut codes = Vec::new();
    let (mut cols, mut rows) = (1, 1);

    while window.is_open() && !window.is_key_down(Key::Escape) && !window.is_key_down(Key::Q) {
        if stream.finished() {
            break;
        }
        if window.is_key_pressed(Key::Space, KeyRepeat::No) {
            paused = !paused;
        }
        if window.is_key_pressed(Key::Up, KeyRepeat::Yes)
            || window.is_key_pressed(Key::Equal, KeyRepeat::Yes)
        {
            fps = (fps * 1.25).min(60.0);
        }
        if window.is_key_pressed(Key::Down, KeyRepeat::Yes)
            || window.is_key_pressed(Key::Minus, KeyRepeat::Yes)
        {
            fps = (fps / 1.25).max(0.5);
        }
        let (w, h) = window.get_size();
        if !paused && Instant::now() >= next {
            (cols, rows) = grid.resolve(modules, w, h, min_scale);
            codes = stream.next_matrices(cols * rows)?;
            dirty = true;
            next += Duration::from_secs_f64(1.0 / fps);
            if next < Instant::now() {
                next = Instant::now();
            }
        }
        if dirty || buf.len() != w * h {
            buf.clear();
            buf.resize(w * h, WHITE);
            layout_grid(&codes, cols, rows, w, h, |x, y, cw, ch| {
                for row in y..(y + ch).min(h) {
                    let end = (x + cw).min(w);
                    if x < end {
                        buf[row * w + x..row * w + end].fill(BLACK);
                    }
                }
            });
            window.update_with_buffer(&buf, w, h)?;
            dirty = false;
        } else {
            window.update();
        }
        if last_title.elapsed() >= Duration::from_millis(500) {
            let state = if paused { "PAUSED · " } else { "" };
            window.set_title(&format!(
                "QRSend — {state}{}",
                stream.status(fps, cols * rows)
            ));
            last_title = Instant::now();
        }
    }
    Ok(())
}

/// Gives the window QRSend's icon in the Dock. A program that opens a window
/// without being an application bundle (as one run from a terminal is) gets
/// the system's stand-in there, the dark square that says "exec".
#[cfg(target_os = "macos")]
fn dock_icon() {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    // The artwork (assets/qrsendIcon-1024@1x.png) with the margin the
    // system's own icons keep: the system shows what it is handed as it is.
    // Made by scripts/dock-icon.sh.
    static ICON: &[u8] = include_bytes!("../../assets/dock-icon.png");

    let classes = (
        AnyClass::get(c"NSData"),
        AnyClass::get(c"NSImage"),
        AnyClass::get(c"NSApplication"),
    );
    let (Some(data), Some(image), Some(app)) = classes else {
        return;
    };
    // (The window is there, so this is the main thread and the application
    // exists. What is made here is released here: the application keeps the
    // image it is given.)
    unsafe {
        let data: *mut AnyObject = msg_send![data, alloc];
        let data: *mut AnyObject = msg_send![
            data,
            initWithBytes: ICON.as_ptr().cast::<std::ffi::c_void>(),
            length: ICON.len(),
        ];
        let image: *mut AnyObject = msg_send![image, alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if !image.is_null() {
            let app: *mut AnyObject = msg_send![app, sharedApplication];
            let _: () = msg_send![app, setApplicationIconImage: image];
            let _: () = msg_send![image, release];
        }
        let _: () = msg_send![data, release];
    }
}
