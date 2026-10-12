//! `qrsend id` and `qrsend devices`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::time::{Duration, Instant};

use qrsend_core::crypto::{DevicePublic, IdRead, IdReader};
use qrsend_core::qr::{self, Ec, QrMatrix, QrParams};

use crate::display::terminal;
use crate::identity::{self, Added};
use crate::util;

#[derive(Args)]
pub struct IdArgs {
    /// Name for this device (used when the identity is first created)
    #[arg(long)]
    pub name: Option<String>,
    /// Rename this device
    #[arg(long, value_name = "NAME", conflicts_with = "name")]
    pub rename: Option<String>,
    /// Do not draw the ID as a QR code
    #[arg(long)]
    pub no_qr: bool,
    /// Write the ID as a QR code PNG. An ID with post-quantum keys takes
    /// several codes: they are written as FILE-1.png, FILE-2.png, …
    #[arg(long, value_name = "FILE")]
    pub png: Option<PathBuf>,
    /// Show the ID as versions before post-quantum keys understand it (one
    /// short code, the older keys only). What is sent to a device that was
    /// given this ID is encrypted the older way
    #[arg(long)]
    pub classic: bool,
}

#[derive(Subcommand)]
pub enum DevicesCmd {
    /// List trusted devices
    List,
    /// Trust a device: paste its ID (from `qrsend id`) or scan it from images
    Add {
        /// The device ID (qrsend-id:…)
        id: Option<String>,
        /// Read the ID from the QR code in an image, or from the several
        /// codes of an ID with post-quantum keys in several images (in any
        /// order; one picture may hold more than one code)
        #[arg(long, value_name = "FILE", num_args = 1.., conflicts_with = "id")]
        image: Vec<PathBuf>,
        /// Store it under this name
        #[arg(long)]
        name: Option<String>,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
    /// Forget a trusted device (by name or fingerprint)
    Rm { device: String },
    /// Rename a trusted device
    Rename { old: String, new: String },
}

fn print_qr(text: &str) {
    let Ok(m) = qr::render_text(text) else { return };
    let quiet = 2isize;
    let w = m.width as isize;
    let dark =
        |x: isize, y: isize| x >= 0 && y >= 0 && x < w && y < w && m.dark(x as usize, y as usize);
    let mut y = -quiet;
    while y < w + quiet {
        let mut line = String::new();
        for x in -quiet..w + quiet {
            line.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => ' ',
                (true, false) => '▄',
                (false, true) => '▀',
                (false, false) => '█',
            });
        }
        println!("{line}");
        y += 2;
    }
}

/// The size of the codes an ID with post-quantum keys is shown as: small
/// enough to read well and to fit a terminal (81 columns by 42 rows), which
/// makes about nine of them. (At the size of a transfer's codes it would
/// be three, of 121 columns.)
pub(crate) const ID_CODES: QrParams = QrParams {
    version: 15,
    ec: Ec::M,
};
/// How long each code is shown before the next.
const ID_CODE_TIME: Duration = Duration::from_millis(700);

/// Shows the codes of an ID one after the other, round and round, until a
/// key ends it. Space holds the code that is showing and lets go again; the
/// arrow keys step by hand.
pub(crate) fn show_codes(what: &str, codes: &[QrMatrix]) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::style::Print;
    use crossterm::{cursor, queue, terminal as term};

    let (columns, rows) = terminal::cells(codes[0].width);
    let (have_columns, have_rows) = term::size()?;
    if (have_columns as usize) < columns || (have_rows as usize) < rows + 1 {
        bail!(
            "the terminal is too small for the codes: they take {columns}×{} cells, and it \
             has {have_columns}×{have_rows}. Enlarge the window or shrink the font; or write them \
             to pictures where that is offered, or copy the text above",
            rows + 1
        );
    }
    let _guard = terminal::Guard::enter()?;
    let mut out = std::io::stdout();
    let (mut at, mut playing) = (0usize, true);
    let mut next = Instant::now() + ID_CODE_TIME;
    let mut drawn = usize::MAX;
    let mut was_playing = false;
    loop {
        if drawn != at || was_playing != playing {
            terminal::draw(&mut out, &codes[at])?;
            queue!(
                out,
                cursor::MoveTo(0, rows as u16),
                term::Clear(term::ClearType::CurrentLine),
                Print(format!(
                    "{what}: code {} of {}{}  [space] {}  [←/→] step  [q] done",
                    at + 1,
                    codes.len(),
                    if playing { "" } else { " · held" },
                    if playing { "hold" } else { "go on" },
                ))
            )?;
            out.flush()?;
            (drawn, was_playing) = (at, playing);
        }
        let wait = match playing {
            true => next.saturating_duration_since(Instant::now()),
            false => Duration::from_millis(200),
        };
        if event::poll(wait)? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                let step = |at: usize, by: usize| (at + by) % codes.len();
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char(' ') => {
                        playing = !playing;
                        next = Instant::now() + ID_CODE_TIME;
                    }
                    // (A step by hand holds the code stepped to.)
                    KeyCode::Right | KeyCode::Down | KeyCode::Char('n') => {
                        (at, playing) = (step(at, 1), false);
                    }
                    KeyCode::Left | KeyCode::Up | KeyCode::Char('p') => {
                        (at, playing) = (step(at, codes.len() - 1), false);
                    }
                    _ => {}
                }
            }
        } else if playing && Instant::now() >= next {
            at = (at + 1) % codes.len();
            next += ID_CODE_TIME;
        }
    }
    Ok(())
}

pub fn id(args: IdArgs) -> Result<()> {
    let (mut me, created) = identity::load_or_create(args.name.as_deref())?;
    if let Some(name) = args.rename {
        me.name = name;
        identity::save(&me)?;
    }
    let public = match args.classic {
        true => me.public().classic(),
        false => me.public(),
    };
    let id = public.to_id_string();
    if created {
        eprintln!("Created a new device identity.");
    }
    println!("Device:      {}", public.name);
    println!("Fingerprint: {}", public.fingerprint());
    println!(
        "Keys:        {}",
        match public.pq {
            Some(_) => "post-quantum (X25519 + ML-KEM-768, Ed25519 + ML-DSA-65)",
            None => "OLDER ONLY (X25519, Ed25519)",
        }
    );
    println!("ID:          {id}");
    // One code of text for the older kind of ID; several for one with
    // post-quantum keys, which is some 3 kB of keys.
    let texts = public.id_codes(ID_CODES.capacity_chars())?;
    let codes: Vec<QrMatrix> = match texts.as_slice() {
        [text] => vec![qr::render_text(text)?],
        texts => texts
            .iter()
            .map(|t| qr::render(t, ID_CODES))
            .collect::<Result<_, _>>()?,
    };
    if let Some(path) = &args.png {
        for (n, code) in codes.iter().enumerate() {
            let path = match codes.len() {
                1 => path.clone(),
                _ => {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    path.with_file_name(format!("{stem}-{}.png", n + 1))
                }
            };
            let (side, px) = qr::rasterize(code, 8, 4);
            image::GrayImage::from_raw(side as u32, side as u32, px)
                .unwrap()
                .save(&path)?;
            eprintln!("Wrote {}", path.display());
        }
    }
    let on_terminal = !args.no_qr && std::io::stdout().is_terminal();
    if let ([text], true) = (texts.as_slice(), on_terminal) {
        println!();
        print_qr(text);
    }
    eprintln!();
    eprintln!("On a device that will send to this one, run `qrsend devices add <ID>`");
    eprintln!("and check that it shows the same fingerprint.");
    if args.classic && me.has_pq() {
        eprintln!(
            "WARNING: this is the ID for versions before post-quantum keys. What a device given \
             this ID sends here is encrypted the older way only (no protection against a quantum \
             computer). Whatever can read the full ID (`qrsend id`) should be given that one."
        );
    }
    if codes.len() > 1 && on_terminal {
        if !std::io::stdin().is_terminal() {
            return Ok(());
        }
        eprintln!();
        eprint!(
            "The ID takes {} codes, shown one after the other. Press Enter to show them \
             (or q to leave it at the text above): ",
            codes.len()
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("q") {
            show_codes("ID", &codes)?;
        }
    }
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// Reads a device ID from pictures of its code, or of its codes.
fn read_id_from_images(paths: &[PathBuf]) -> Result<DevicePublic> {
    let mut reader = IdReader::default();
    let mut progress = None;
    for path in paths {
        let img = image::open(path)
            .with_context(|| format!("cannot read {}", path.display()))?
            .into_luma8();
        let frame = crate::input::LumaFrame {
            width: img.width() as usize,
            height: img.height() as usize,
            pixels: img.into_raw(),
        };
        for text in crate::decode::detect(&frame) {
            match reader.add(&text) {
                IdRead::Whole(device) => return Ok(*device),
                IdRead::Part { have, of } => progress = Some((have, of)),
                IdRead::Other => {}
            }
        }
    }
    match progress {
        Some((have, of)) => bail!(
            "{have} of the {of} codes of a device ID were found in the pictures: all of them are needed"
        ),
        None => bail!("no QRSend device ID found in the pictures"),
    }
}

pub fn devices(cmd: DevicesCmd) -> Result<()> {
    match cmd {
        DevicesCmd::List => {
            let list = identity::devices()?;
            if list.is_empty() {
                println!("No trusted devices. Add one with `qrsend devices add <ID>`.");
            }
            for d in list {
                let public = d.public()?;
                println!(
                    "{:<24} {}{}",
                    util::printable(&d.name),
                    public.fingerprint(),
                    match public.pq {
                        Some(_) => "  post-quantum keys",
                        None => "  OLDER KEYS ONLY: no post-quantum protection (add its ID again)",
                    }
                );
            }
        }
        DevicesCmd::Add {
            id,
            image,
            name,
            yes,
        } => {
            let public = match (id, image.as_slice()) {
                (Some(id), _) => DevicePublic::parse(&id)?,
                (None, []) => bail!("give the device ID, or --image FILE with its QR code"),
                (None, images) => read_id_from_images(images)?,
            };
            let shown = name.as_deref().unwrap_or(&public.name);
            eprintln!("Device:      {}", util::printable(shown));
            eprintln!("Fingerprint: {}", public.fingerprint());
            if !yes {
                if !std::io::stdin().is_terminal() {
                    bail!(
                        "refusing to add without confirmation; compare the fingerprint and pass --yes"
                    );
                }
                if !confirm("Does the other device show the same fingerprint?")? {
                    bail!("not added");
                }
            }
            if public.pq.is_none() {
                eprintln!(
                    "WARNING: this is an ID without post-quantum keys: what is sent to this \
                     device will be encrypted the older way only, and its signature checked the \
                     older way only. (A device on a current version shows the full ID.)"
                );
            }
            match identity::add_device(&public, name.as_deref())? {
                Added::New(t) => println!(
                    "Trusted {}. Send to it with `qrsend send --to {:?} …`",
                    util::printable(&t.name),
                    t.name
                ),
                Added::Upgraded(t) => println!(
                    "{} was trusted already; it now has post-quantum keys here as well.",
                    util::printable(&t.name)
                ),
            }
        }
        DevicesCmd::Rm { device } => {
            let mut list = identity::devices()?;
            let before = list.len();
            list.retain(|d| {
                d.name != device
                    && d.public()
                        .map(|p| p.fingerprint() != device && p.classic().fingerprint() != device)
                        .unwrap_or(true)
            });
            if list.len() == before {
                bail!("no trusted device {device:?}");
            }
            identity::save_devices(&list)?;
            println!("Removed {}.", util::printable(&device));
        }
        DevicesCmd::Rename { old, new } => {
            let mut list = identity::devices()?;
            if list.iter().any(|d| d.name == new) {
                bail!("a device is already named {new:?}");
            }
            let d = list
                .iter_mut()
                .find(|d| d.name == old)
                .with_context(|| format!("no trusted device {old:?}"))?;
            d.name = new.clone();
            identity::save_devices(&list)?;
            println!("Renamed {old} to {new}.");
        }
    }
    Ok(())
}
