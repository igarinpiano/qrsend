//! Shows the stream in a native window (largest, densest option).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use minifb::{Key, KeyRepeat, ScaleMode, Window, WindowOptions};

use super::{FrameStream, GridSpec};

/// `min_scale` is the smallest module size (pixels) an automatic grid may
/// use. `tuned`: pictures per second and codes per picture follow what the
/// receiver reports (`--auto-speed`), starting from `fps` and one code.
pub fn run(
    stream: &mut FrameStream,
    mut fps: f64,
    grid: GridSpec,
    min_scale: usize,
    tuned: bool,
) -> Result<()> {
    let opts = WindowOptions {
        resize: true,
        scale_mode: ScaleMode::UpperLeft,
        ..WindowOptions::default()
    };
    let mut window = Window::new("QRSend", 900, 900, opts)
        .context("cannot open a window (try --display terminal)")?;
    #[cfg(target_os = "macos")]
    {
        dock_icon();
        quit_ends_the_loop();
    }
    window.set_target_fps(120);

    let modules = stream.params.modules();
    let mut buf: Vec<u32> = Vec::new();
    let mut paused = false;
    let mut next = Instant::now();
    let mut last_title = Instant::now() - Duration::from_secs(2);
    let mut dirty = true;
    let mut codes = Vec::new();
    let (mut cols, mut rows) = (1, 1);
    // The colors the codes on the screen are stacked in (0: black and white).
    let mut colors = 0u8;

    let _watched = super::Watched::new();
    while window.is_open()
        && !window.is_key_down(Key::Escape)
        && !window.is_key_down(Key::Q)
        && !super::stop_asked()
    {
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
            stream.tuned_by_hand(fps);
        }
        if window.is_key_pressed(Key::Down, KeyRepeat::Yes)
            || window.is_key_pressed(Key::Minus, KeyRepeat::Yes)
        {
            fps = (fps / 1.25).max(0.5);
            stream.tuned_by_hand(fps);
        }
        let (w, h) = window.get_size();
        if !paused && Instant::now() >= next {
            colors = stream.colors();
            let layers = colors.count_ones().max(1) as usize;
            if tuned {
                // What the window offers at its size now: every layout from
                // one code to as many as fit, or the one asked for.
                let offered = match grid {
                    GridSpec::Auto => super::layouts(modules, w, h, min_scale),
                    GridSpec::Fixed { cols, rows } => vec![(cols, rows, 1)],
                };
                let at = offered
                    .iter()
                    .position(|&(c, r, _)| (c, r) == (cols, rows))
                    .unwrap_or(0);
                stream.tune(
                    offered
                        .iter()
                        .map(|&(c, r, _)| (c * r * layers) as u32)
                        .collect(),
                    offered.iter().map(|&(.., dot)| dot as f64).collect(),
                    qrsend_core::tune::Setting { fps, level: at },
                );
                if let Some(setting) = stream.tuned() {
                    fps = setting.fps;
                    (cols, rows, _) = offered[setting.level.min(offered.len() - 1)];
                }
            } else {
                (cols, rows) = grid.resolve(modules, w, h, min_scale);
            }
            codes = stream.next_matrices(cols * rows * layers)?;
            dirty = true;
            next += Duration::from_secs_f64(1.0 / fps);
            if next < Instant::now() {
                next = Instant::now();
            }
        }
        if dirty || buf.len() != w * h {
            buf = super::paint(&codes, colors, cols, rows, w, h);
            window.update_with_buffer(&buf, w, h)?;
            dirty = false;
        } else {
            window.update();
        }
        if last_title.elapsed() >= Duration::from_millis(500) {
            let state = if paused { "PAUSED · " } else { "" };
            window.set_title(&format!(
                "QRSend — {state}{}",
                stream.status(fps, cols * rows * colors.count_ones().max(1) as usize)
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
pub(super) fn dock_icon() {
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

/// Makes quitting the application (Cmd+Q, "Quit" in the menu or in the Dock)
/// end the loop that shows a window as closing the window does. Left to itself, the
/// application ends the whole program on the spot: nothing after the loop
/// would run, neither the closing lines nor the removal of what is not to
/// be kept.
#[cfg(target_os = "macos")]
pub(super) fn quit_ends_the_loop() {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
    use objc2::{msg_send, sel};

    /// `applicationShouldTerminate:`: no (`NSTerminateCancel`), but the
    /// loop is told, and the program ends by its own means a moment later.
    extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
        super::ask_stop();
        0
    }

    let (Some(object), Some(app)) = (AnyClass::get(c"NSObject"), AnyClass::get(c"NSApplication"))
    else {
        return;
    };
    let class = AnyClass::get(c"QRSendQuit").or_else(|| {
        let mut class = ClassBuilder::new(c"QRSendQuit", object)?;
        // (The method is what the application calls on its delegate: one
        // object in, an unsigned integer out.)
        unsafe {
            class.add_method(
                sel!(applicationShouldTerminate:),
                should_terminate as extern "C-unwind" fn(_, _, _) -> _,
            );
        }
        Some(class.register())
    });
    let Some(class) = class else { return };
    // (The application does not keep its delegate alive; this one lives as
    // long as the program.)
    unsafe {
        let delegate: *mut AnyObject = msg_send![class, new];
        let app: *mut AnyObject = msg_send![app, sharedApplication];
        let _: () = msg_send![app, setDelegate: delegate];
    }
}
