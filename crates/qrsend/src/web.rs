//! The address of QRSend's web app, for a receiver who has no QRSend yet:
//! as an ordinary QR code, which the camera app of any phone opens, and as
//! text to type. It opens the Receive page, ready to read the codes.

use anyhow::Result;
use qrsend_core::qr::{self, QrMatrix};

/// The Receive page of the published web app.
pub const RECEIVE: &str = "https://igarinpiano.github.io/qrsend/#/receive";

/// What to tell someone looking at the code.
pub const HOW: &str = "Point the other device's camera at the code (a phone's own camera app is \
    enough): it opens QRSend in the browser, ready to receive. Nothing to install.";

#[derive(clap::Args)]
pub struct WebArgs {}

/// The code that opens the Receive page.
pub fn code() -> QrMatrix {
    qr::render_text(RECEIVE).expect("a short address fits a QR code")
}

/// `qrsend web`: prints the code and the address.
pub fn run(_args: WebArgs) -> Result<()> {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        print_colored(&code())?;
    } else {
        crate::devices::print_qr(RECEIVE);
    }
    println!("{RECEIVE}");
    eprintln!("{HOW}");
    Ok(())
}

/// Black on white, whatever colors the terminal has (a camera does not
/// read every code with light and dark the other way round), two modules to
/// a character cell, with a margin of four.
fn print_colored(m: &QrMatrix) -> Result<()> {
    use crossterm::queue;
    use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
    use std::io::Write;

    const MARGIN: isize = 4;
    let w = m.width as isize;
    let dark =
        |x: isize, y: isize| x >= 0 && y >= 0 && x < w && y < w && m.dark(x as usize, y as usize);
    let color = |dark: bool| if dark { Color::Black } else { Color::White };
    let mut out = std::io::stdout().lock();
    let mut y = -MARGIN;
    while y < w + MARGIN {
        for x in -MARGIN..w + MARGIN {
            queue!(
                out,
                SetForegroundColor(color(dark(x, y))),
                SetBackgroundColor(color(dark(x, y + 1))),
                Print('▀')
            )?;
        }
        queue!(out, ResetColor, Print('\n'))?;
        y += 2;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_reads_back_as_the_address() {
        let m = code();
        // Each module as a 4×4 block of pixels, with a margin of four.
        let (scale, margin) = (4, 4);
        let side = (m.width + 2 * margin) * scale;
        let mut pixels = vec![255u8; side * side];
        for y in 0..m.width {
            for x in 0..m.width {
                if m.dark(x, y) {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = ((x + margin) * scale + dx, (y + margin) * scale + dy);
                            pixels[py * side + px] = 0;
                        }
                    }
                }
            }
        }
        let frame = crate::input::LumaFrame {
            width: side,
            height: side,
            pixels,
        };
        assert_eq!(crate::decode::detect(&frame), vec![RECEIVE.to_string()]);
    }
}
