//! Shows the stream in the terminal with half-block characters.

use std::io::{Write, stdout};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};

use super::FrameStream;

const QUIET: usize = 2;

struct Guard;

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
    terminal::enable_raw_mode()?;
    let _guard = Guard;
    let mut out = stdout();
    execute!(
        out,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        terminal::Clear(terminal::ClearType::All)
    )?;

    let mut paused = false;
    let mut next = Instant::now();
    loop {
        if !paused && Instant::now() >= next {
            let m = stream.next_matrix()?;
            let dark = |x: isize, y: isize| {
                x >= 0
                    && y >= 0
                    && (x as usize) < m.width
                    && (y as usize) < m.width
                    && m.dark(x as usize, y as usize)
            };
            for row in 0..modules.div_ceil(2) {
                queue!(out, cursor::MoveTo(0, row as u16))?;
                for col in 0..modules {
                    let (x, y) = (
                        col as isize - QUIET as isize,
                        (row * 2) as isize - QUIET as isize,
                    );
                    let top = if dark(x, y) {
                        Color::Black
                    } else {
                        Color::White
                    };
                    let bottom = if dark(x, y + 1) {
                        Color::Black
                    } else {
                        Color::White
                    };
                    queue!(
                        out,
                        SetForegroundColor(top),
                        SetBackgroundColor(bottom),
                        Print('▀')
                    )?;
                }
            }
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
