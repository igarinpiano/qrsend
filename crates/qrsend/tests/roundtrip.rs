//! End-to-end tests of the CLI: send → exported frames → recv.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    root: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("qrsend-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Env { root }
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_as("me", args)
    }

    /// Runs as a given "device" (separate identity, inbox and cache).
    fn run_as(&self, device: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_qrsend"))
            .args(args)
            .current_dir(&self.root)
            .env("QRSEND_DATA_DIR", self.path(&format!("{device}/data")))
            .env("QRSEND_CACHE_DIR", self.path(&format!("{device}/cache")))
            .env("QRSEND_CONFIG_DIR", self.path(&format!("{device}/config")))
            .output()
            .unwrap()
    }

    fn ok_as(&self, device: &str, args: &[&str]) -> String {
        let out = self.run_as(device, args);
        assert!(
            out.status.success(),
            "{device}: qrsend {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "qrsend {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn noise(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn frames(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

#[test]
fn text_roundtrip() {
    let env = Env::new("text");
    env.ok(&[
        "send",
        "--plain",
        "--text",
        "héllo, QR 🌈",
        "--density",
        "low",
        "--export-frames",
        "f",
    ]);
    assert_eq!(
        env.ok(&["recv", "--images", "f"]).trim_end(),
        "héllo, QR 🌈"
    );
}

#[test]
fn folder_roundtrip_with_loss_and_resume() {
    let env = Env::new("folder");
    fs::create_dir_all(env.path("in/docs/deep")).unwrap();
    fs::write(env.path("in/docs/a.bin"), noise(200_000)).unwrap();
    fs::write(env.path("in/docs/deep/b.txt"), "line\n".repeat(10_000)).unwrap();
    fs::write(env.path("in/docs/empty"), "").unwrap();

    env.ok(&[
        "send",
        "--plain",
        "in/docs",
        "--seg-shift",
        "15",
        "--export-frames",
        "f",
    ]);
    // Lose a burst in the middle of the single pass: some segments stay incomplete.
    let all = frames(&env.path("f"));
    let lost = all.len() / 3..all.len() * 2 / 3;
    for f in &all[lost] {
        fs::remove_file(f).unwrap();
    }
    let out = env.run(&["recv", "--images", "f", "-o", "out"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let code = stderr
        .split_whitespace()
        .find(|w| w.starts_with("QSR1-"))
        .expect("resume code")
        .to_string();

    env.ok(&["send", "--resume", &code, "--export-frames", "g"]);
    env.ok(&["recv", "--images", "g", "-o", "out"]);
    assert_eq!(
        fs::read(env.path("out/docs/a.bin")).unwrap(),
        noise(200_000)
    );
    assert_eq!(
        fs::read_to_string(env.path("out/docs/deep/b.txt")).unwrap(),
        "line\n".repeat(10_000)
    );
    assert_eq!(fs::read(env.path("out/docs/empty")).unwrap(), b"");
    assert!(env.ok(&["inbox", "list"]).contains("empty"));
}

#[test]
fn y4m_video_and_conflicts() {
    let env = Env::new("video");
    fs::write(env.path("note.txt"), "first").unwrap();
    env.ok(&[
        "send",
        "--plain",
        "note.txt",
        "--grid",
        "2",
        "--export-y4m",
        "v.y4m",
        "--scale",
        "3",
    ]);
    fs::create_dir_all(env.path("out")).unwrap();
    fs::write(env.path("out/note.txt"), "existing").unwrap();
    env.ok(&["recv", "--video", "v.y4m", "-o", "out"]);
    assert_eq!(
        fs::read_to_string(env.path("out/note.txt")).unwrap(),
        "existing"
    );
    assert_eq!(
        fs::read_to_string(env.path("out/note (1).txt")).unwrap(),
        "first"
    );
}

fn device_id(env: &Env, device: &str) -> String {
    let out = env.ok_as(device, &["id", "--name", device, "--no-qr"]);
    out.lines()
        .find_map(|l| l.strip_prefix("ID:"))
        .unwrap()
        .trim()
        .to_string()
}

#[test]
fn encrypted_transfer_between_paired_devices() {
    let env = Env::new("crypto");
    let (alice, bob) = (device_id(&env, "alice"), device_id(&env, "bob"));
    device_id(&env, "eve");
    // Plain sends need an explicit choice.
    assert!(
        !env.run_as("alice", &["send", "--text", "x", "--export-frames", "nope"])
            .status
            .success()
    );

    env.ok_as("alice", &["devices", "add", &bob, "--yes"]);
    env.ok_as("bob", &["devices", "add", &alice, "--yes"]);
    fs::write(env.path("secret.txt"), "for bob only").unwrap();
    env.ok_as(
        "alice",
        &["send", "secret.txt", "--to", "bob", "--export-frames", "f"],
    );

    let eve = env.run_as("eve", &["recv", "--images", "f", "-o", "eve-out"]);
    assert!(!eve.status.success());
    assert!(String::from_utf8_lossy(&eve.stderr).contains("encrypted for another device"));

    let out = env.run_as("bob", &["recv", "--images", "f", "-o", "bob-out"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("alice ✓"));
    assert_eq!(
        fs::read_to_string(env.path("bob-out/secret.txt")).unwrap(),
        "for bob only"
    );

    // Pairing by scanning the ID QR code from an image.
    let id_png = env.path("id.png");
    env.ok_as("bob", &["id", "--no-qr", "--png", id_png.to_str().unwrap()]);
    env.ok_as("eve", &["devices", "add", "--image", "id.png", "--yes"]);
    assert!(env.ok_as("eve", &["devices", "list"]).contains("bob"));
}

#[test]
fn dense_video_roundtrip() {
    let env = Env::new("dense");
    fs::write(env.path("data.bin"), noise(400_000)).unwrap();
    // Many small codes per frame, automatic grid and code size.
    let out = env.run(&[
        "send",
        "--plain",
        "data.bin",
        "--dense",
        "--size",
        "1280x720",
        "--export-video",
        "dense.y4m",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("1280×720 px"), "{stderr}");
    env.ok(&["recv", "--video", "dense.y4m", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(400_000));
}

#[test]
fn large_fixed_grid_and_auto_density() {
    let env = Env::new("grid");
    // A tiny transfer picks small codes on its own.
    let out = env.run(&[
        "send",
        "--plain",
        "--text",
        "hi",
        "--export-frames",
        "small",
    ]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("QR v10-M"));
    assert_eq!(env.ok(&["recv", "--images", "small"]).trim_end(), "hi");

    fs::write(env.path("data.bin"), noise(150_000)).unwrap();
    env.ok(&[
        "send",
        "--plain",
        "data.bin",
        "--grid",
        "6x4",
        "--density",
        "low",
        "--scale",
        "2",
        "--export-frames",
        "f",
    ]);
    env.ok(&["recv", "--images", "f", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(150_000));
}

fn has_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn compressed_video_roundtrip_through_ffmpeg() {
    if !has_ffmpeg() {
        eprintln!("ffmpeg not installed; skipping");
        return;
    }
    let env = Env::new("mp4");
    fs::write(env.path("data.bin"), noise(200_000)).unwrap();
    env.ok(&[
        "send",
        "--plain",
        "data.bin",
        "--dense",
        "--size",
        "1280x720",
        "--passes",
        "1.5",
        "--export-video",
        "dense.mp4",
    ]);
    env.ok(&["recv", "--video", "dense.mp4", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(200_000));
}

#[test]
fn text_frames_through_a_lossy_byte_channel() {
    let env = Env::new("pipe");
    fs::write(env.path("data.bin"), noise(300_000)).unwrap();
    env.ok(&[
        "send",
        "--plain",
        "data.bin",
        "--seg-shift",
        "16",
        "--passes",
        "1.3",
        "--export-text",
        "frames.txt",
    ]);
    // The channel loses every tenth line, garbles some and adds noise of its own.
    let text = fs::read_to_string(env.path("frames.txt")).unwrap();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        match i % 10 {
            3 => {}
            6 => out.extend_from_slice(&line.as_bytes()[..line.len() / 2]),
            _ => out.extend_from_slice(line.as_bytes()),
        }
        out.extend_from_slice(if i % 4 == 0 { b"\r\n" } else { b"\n" });
        if i % 25 == 0 {
            out.extend_from_slice(b"login: \xff\xfe garbage\n\n");
        }
    }
    fs::write(env.path("received.txt"), out).unwrap();
    env.ok(&["recv", "--text", "received.txt", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(300_000));
}

#[test]
fn a_network_connection_carries_what_the_codes_did_not() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let env = Env::new("lan");
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let data: Vec<u8> = (0..3_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    fs::write(env.path("big.bin"), &data).unwrap();

    // The sender writes a few codes (a tiny part of the transfer, with the
    // offer to connect among them) and then serves whoever connects.
    let mut sender = Command::new(env!("CARGO_BIN_EXE_qrsend"))
        .args([
            "send",
            "big.bin",
            "--plain",
            "--no-compress",
            "--lan",
            "--lan-address",
            "127.0.0.1",
            "--export-frames",
            "frames",
            "--frames",
            "60",
            "--log",
            "send.log",
        ])
        .current_dir(&env.root)
        .env("QRSEND_DATA_DIR", env.path("me/data"))
        .env("QRSEND_CACHE_DIR", env.path("me/cache"))
        .env("QRSEND_CONFIG_DIR", env.path("me/config"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (said, heard) = mpsc::channel::<String>();
    let stderr = sender.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = said.send(line);
        }
    });
    let wait_for = |what: &str| {
        loop {
            let line = heard
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("the sender never said {what:?}"));
            if line.contains(what) {
                return;
            }
        }
    };
    wait_for("Waiting for the receiver on the network");

    // A receiver that keeps to the codes gets what 60 codes hold: not much.
    let alone = env.run_as(
        "other",
        &["recv", "--images", "frames", "--no-lan", "-o", "alone"],
    );
    assert_eq!(alone.status.code(), Some(2));
    assert!(!env.path("alone/big.bin").exists());

    // One that takes the offer gets everything through the connection.
    let out = env.run_as(
        "them",
        &[
            "recv", "--images", "frames", "-o", "out", "--log", "recv.log",
        ],
    );
    let log = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{log}");
    assert!(log.contains("Receiving over the network."), "{log}");
    assert_eq!(fs::read(env.path("out/big.bin")).unwrap(), data);

    // And the sender, told so, stops by itself.
    wait_for("The receiver has everything.");
    assert!(sender.wait().unwrap().success());

    // The diagnostic logs tell what happened, and over what kind of path,
    // without a file name or an address in them.
    let sent = fs::read_to_string(env.path("send.log")).unwrap();
    let received = fs::read_to_string(env.path("recv.log")).unwrap();
    assert!(sent.contains("tx     packed wireBytes="), "{sent}");
    assert!(
        sent.contains(
            "TCP: a receiver connected own=\"v4 loopback\" other=\"v4 loopback\" over=local"
        ),
        "{sent}"
    );
    assert!(sent.contains("the receiver has everything"), "{sent}");
    assert!(received.contains("input  from=images"), "{received}");
    assert!(
        received.contains("rx     transfer found segments=4"),
        "{received}"
    );
    assert!(
        received.contains("TCP: connected own=\"v4 loopback\""),
        "{received}"
    );
    assert!(received.contains("input over complete=true"), "{received}");
    for log in [&sent, &received] {
        assert!(log.starts_with("QRSend CLI "), "{log}");
        assert!(
            !log.contains("big.bin") && !log.contains("127.0.0.1"),
            "{log}"
        );
    }
}

/// A file list that does not open (damaged, or someone else's codes with the
/// same session id) is collected again instead of ending the transfer, and
/// is not stored where it would stop every later attempt.
#[test]
fn a_damaged_file_list_is_received_again() {
    use qrsend_core::frame::Frame;

    let env = Env::new("badmeta");
    env.ok(&[
        "send",
        "--plain",
        "--text",
        "the real text",
        "--passes",
        "2",
        "--export-text",
        "frames.txt",
    ]);
    let text = fs::read_to_string(env.path("frames.txt")).unwrap();
    let meta = text
        .lines()
        .filter_map(|l| Frame::from_qr_text(l).ok())
        .find(|f| f.header.is_meta())
        .expect("a meta frame");
    // The same meta segment, with other content: every source symbol of it.
    let size = meta.symbol.len();
    let k = (meta.header.seg_len as usize).div_ceil(size);
    let mut forged = String::new();
    for esi in 0..k {
        let header = qrsend_core::frame::FrameHeader {
            esi: esi as u32,
            ..meta.header
        };
        let symbol = noise(size * (esi + 1))[size * esi..].to_vec();
        forged.push_str(&Frame { header, symbol }.to_qr_text());
        forged.push('\n');
    }
    fs::write(env.path("received.txt"), format!("{forged}{text}")).unwrap();
    let out = env.run(&["recv", "--text", "received.txt"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("file list failed verification"), "{stderr}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        "the real text"
    );
}

/// What a transfer names is shown, not obeyed: a file name or a sender's
/// name cannot make the terminal do anything.
#[test]
fn names_cannot_drive_the_terminal() {
    let env = Env::new("escapes");
    fs::create_dir_all(env.path("in")).unwrap();
    let name = "report\u{1b}]0;owned\u{7}.txt";
    if fs::write(env.path("in").join(name), "x").is_err() {
        return; // (A file system that refuses such a name.)
    }
    env.ok(&[
        "send",
        "--plain",
        "--sender-name",
        "evil\u{1b}[2J",
        &format!("in/{name}"),
        "--export-text",
        "frames.txt",
    ]);
    let out = env.run(&["recv", "--text", "frames.txt", "--keep", "-o", "out"]);
    assert!(out.status.success());
    let shown = String::from_utf8_lossy(&out.stderr);
    assert!(!shown.contains('\u{1b}'), "{shown:?}");
    let list = env.ok(&["inbox", "list"]);
    assert!(
        !list.contains('\u{1b}') && list.contains("report\\u{1b}"),
        "{list:?}"
    );
}
