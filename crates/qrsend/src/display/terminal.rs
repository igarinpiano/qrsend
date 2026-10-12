//! Shows the stream in the terminal with half-block characters.

use std::io::{Write, stdout};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};

use super::FrameStream;

const QUIET: usize = 2;

/// The terminal as a screen for codes, for as long as this lives: raw
/// input, its own page, no cursor.
pub struct Guard;

impl Guard {
    pub fn enter() -> Result<Guard> {
        terminal::enable_raw_mode()?;
        let guard = Guard;
        execute!(
            stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All)
        )?;
        Ok(guard)
    }
}

/// Rows a code of `modules` width takes, and the columns (with its margin).
pub fn cells(modules: usize) -> (usize, usize) {
    let side = modules + 2 * QUIET;
    (side, side.div_ceil(2))
}

/// Draws a code at the top left, two modules to a character cell.
pub fn draw(out: &mut impl Write, m: &qrsend_core::qr::QrMatrix) -> Result<()> {
    let (columns, rows) = cells(m.width);
    let dark = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < m.width
            && (y as usize) < m.width
            && m.dark(x as usize, y as usize)
    };
    for row in 0..rows {
        queue!(out, cursor::MoveTo(0, row as u16))?;
        for col in 0..columns {
            let (x, y) = (
                col as isize - QUIET as isize,
                (row * 2) as isize - QUIET as isize,
            );
            let color = |dark: bool| if dark { Color::Black } else { Color::White };
            queue!(
                out,
                SetForegroundColor(color(dark(x, y))),
                SetBackgroundColor(color(dark(x, y + 1))),
                Print('▀')
            )?;
        }
    }
    queue!(out, ResetColor)?;
    Ok(())
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = execute!(
            stdout(),
            ResetColor,
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run(stream: &mut FrameStream, mut fps: f64) -> Result<()> {
    let modules = stream.params.modules() + 2 * QUIET;
    let (cols, rows) = terminal::size()?;
    let need_rows = modules.div_ceil(2) + 1;
    if (cols as usize) < modules || (rows as usize) < need_rows {
        bail!(
            "terminal too small: need {modules}×{need_rows} cells, have {cols}×{rows}. \
             Enlarge the window / shrink the font, or use --density low or --display window"
        );
    }
    let _guard = Guard::enter()?;
    let mut out = stdout();

    let mut paused = false;
    let mut next = Instant::now();
    loop {
        if stream.finished() {
            break;
        }
        if !paused && Instant::now() >= next {
            let m = stream.next_matrix()?;
            draw(&mut out, &m)?;
            queue!(
                out,
                ResetColor,
                cursor::MoveTo(0, modules.div_ceil(2) as u16),
                terminal::Clear(terminal::ClearType::CurrentLine)
            )?;
            queue!(
                out,
                Print(format!(
                    "{}  [space] pause  [+/-] speed  [q] quit",
                    stream.status(fps, 1)
                ))
            )?;
            out.flush()?;
            next += Duration::from_secs_f64(1.0 / fps);
            if next < Instant::now() {
                next = Instant::now();
            }
        }
        let wait = next
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        if event::poll(wait)?
            && let Event::Key(k) = event::read()?
        {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char(' ') => paused = !paused,
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Up => {
                    fps = (fps * 1.25).min(60.0)
                }
                KeyCode::Char('-') | KeyCode::Down => fps = (fps / 1.25).max(0.5),
                _ => {}
            }
        }
    }
    Ok(())
}
