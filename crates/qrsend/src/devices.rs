//! `qrsend id` and `qrsend devices`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use qrsend_core::crypto::DevicePublic;
use qrsend_core::qr::{self, Luma};

use crate::{identity, util};

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
    /// Write the ID as a QR code PNG
    #[arg(long, value_name = "FILE")]
    pub png: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum DevicesCmd {
    /// List trusted devices
    List,
    /// Trust a device: paste its ID (from `qrsend id`) or scan it from an image
    Add {
        /// The device ID (qrsend-id:1:…)
        id: Option<String>,
        /// Read the ID from a QR code in this image
        #[arg(long, value_name = "FILE", conflicts_with = "id")]
        image: Option<PathBuf>,
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

pub fn id(args: IdArgs) -> Result<()> {
    let (mut me, created) = identity::load_or_create(args.name.as_deref())?;
    if let Some(name) = args.rename {
        me.name = name;
        identity::save(&me)?;
    }
    let public = me.public();
    let id = public.to_id_string();
    if created {
        eprintln!("Created a new device identity.");
    }
    println!("Device:      {}", public.name);
    println!("Fingerprint: {}", public.fingerprint());
    println!("ID:          {id}");
    if let Some(path) = &args.png {
        let m = qr::render_text(&id)?;
        let (side, px) = qr::rasterize(&m, 8, 4);
        image::GrayImage::from_raw(side as u32, side as u32, px)
            .unwrap()
            .save(path)?;
        eprintln!("Wrote {}", path.display());
    }
    if !args.no_qr && std::io::stdout().is_terminal() {
        println!();
        print_qr(&id);
    }
    eprintln!();
    eprintln!("On a device that will send to this one, run `qrsend devices add <ID>`");
    eprintln!("and check that it shows the same fingerprint.");
    Ok(())
}

fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn read_id_from_image(path: &PathBuf) -> Result<String> {
    let img = image::open(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .into_luma8();
    let found = qr::detect(Luma {
        width: img.width() as usize,
        height: img.height() as usize,
        pixels: img.as_raw(),
    });
    found
        .into_iter()
        .find(|t| t.starts_with("qrsend-id:"))
        .context("no QRSend device ID found in the image")
}

pub fn devices(cmd: DevicesCmd) -> Result<()> {
    match cmd {
        DevicesCmd::List => {
            let list = identity::devices()?;
            if list.is_empty() {
                println!("No trusted devices. Add one with `qrsend devices add <ID>`.");
            }
            for d in list {
                println!(
                    "{:<24} {}",
                    util::printable(&d.name),
                    d.public()?.fingerprint()
                );
            }
        }
        DevicesCmd::Add {
            id,
            image,
            name,
            yes,
        } => {
            let id = match (id, image) {
                (Some(id), _) => id,
                (None, Some(path)) => read_id_from_image(&path)?,
                (None, None) => bail!("give the device ID, or --image FILE with its QR code"),
            };
            let public = DevicePublic::parse(&id)?;
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
            let t = identity::add_device(&public, name.as_deref())?;
            println!(
                "Trusted {}. Send to it with `qrsend send --to {:?} …`",
                util::printable(&t.name),
                t.name
            );
        }
        DevicesCmd::Rm { device } => {
            let mut list = identity::devices()?;
            let before = list.len();
            list.retain(|d| {
                d.name != device
                    && d.public()
                        .map(|p| p.fingerprint() != device)
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
