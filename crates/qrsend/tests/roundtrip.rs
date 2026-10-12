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
        self.command(device, args).output().unwrap()
    }

    /// The command `run_as` runs, to be given more of an environment.
    fn command(&self, device: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_qrsend"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("QRSEND_DATA_DIR", self.path(&format!("{device}/data")))
            .env("QRSEND_CACHE_DIR", self.path(&format!("{device}/cache")))
            .env("QRSEND_CONFIG_DIR", self.path(&format!("{device}/config")))
            .env_remove("QRSEND_LOG")
            .env_remove("QRSEND_SECURE")
            .env_remove("QRSEND_NO_KEEP");
        command
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
    // The same where the storage is listed: of what was received, and of
    // what was sent.
    let list = env.ok(&["storage"]);
    assert!(
        !list.contains('\u{1b}') && list.matches("report\\u{1b}").count() == 2,
        "{list:?}"
    );
}

#[test]
fn storage_and_logs_are_listed_and_removed() {
    let env = Env::new("storage");
    fs::create_dir_all(env.path("in")).unwrap();
    fs::write(env.path("in/secret-name.bin"), noise(300_000)).unwrap();

    // Nothing yet.
    assert!(
        env.ok(&["storage"])
            .starts_with("QRSend keeps nothing on this machine.")
    );
    let out = env.run(&["log"]);
    assert!(out.status.success() && out.stdout.is_empty());

    // A send, and a receive that stops halfway.
    env.ok(&["send", "--plain", "in", "--export-frames", "f"]);
    let all = frames(&env.path("f"));
    for f in &all[40..] {
        fs::remove_file(f).unwrap();
    }
    let out = env.run(&["recv", "--images", "f", "-o", "out"]);
    assert_eq!(out.status.code(), Some(2));

    // Both left a log without having been asked, and the latest is printed.
    let list = env.ok(&["log", "list"]);
    assert_eq!(list.matches(" UTC  ").count(), 2, "{list}");
    let received = env.ok(&["log"]);
    assert!(received.contains("· qrsend recv\n"), "{received}");
    assert!(
        received.contains("end    not complete: kept in the inbox"),
        "{received}"
    );
    // Runs are counted back from the latest, as git counts commits.
    assert_eq!(env.ok(&["log", "HEAD"]), received);
    let sent = env.ok(&["log", "HEAD~1"]);
    assert!(sent.contains("· qrsend send\n"), "{sent}");
    assert!(sent.trim_end().ends_with("end    finished"), "{sent}");
    assert_eq!(env.ok(&["log", "show", "HEAD^"]), sent);
    assert!(
        list.contains("HEAD     ") && list.contains("HEAD~1   "),
        "{list}"
    );
    assert!(!sent.contains("secret-name") && !received.contains("secret-name"));
    assert!(!env.run(&["log", "HEAD~2"]).status.success());
    assert!(!env.run(&["log", "2"]).status.success());

    // A run that fails says that there is a log, and the log says how it ended.
    let out = env.run(&["recv", "--images", "nowhere"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("`qrsend log` prints it"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let failed = env.ok(&["log"]);
    assert!(failed.contains("end    stopped with an error"), "{failed}");
    assert!(!failed.contains("nowhere"), "{failed}");

    // No log is kept when it is written elsewhere, or turned off.
    env.ok(&[
        "send",
        "--plain",
        "--text",
        "x",
        "--export-frames",
        "g",
        "--log",
        "own.log",
    ]);
    let out = env
        .command(
            "me",
            &["send", "--plain", "--text", "x", "--export-frames", "h"],
        )
        .env("QRSEND_LOG", "off")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(env.path("own.log").exists());
    assert_eq!(env.ok(&["log", "list"]).matches(" UTC  ").count(), 3);

    // Everything is in the list: the unfinished transfer, the three sent, the logs.
    let listed = env.ok(&["storage"]);
    assert!(listed.contains("1/2 parts"), "{listed}");
    assert!(listed.contains("1 file, 292.97 KiB"), "{listed}");
    assert_eq!(listed.matches("removed in 14 days").count(), 3, "{listed}");
    assert!(listed.contains("3 logs"), "{listed}");

    // Nothing is removed without a word of what, or without a yes.
    for args in [
        &["storage", "clean"][..],
        &["storage", "clean", "--inbox"],
        &["storage", "clean", "nothing-like-this", "--yes"],
    ] {
        assert!(!env.run(args).status.success(), "{args:?}");
    }
    let dry = env.ok(&["storage", "clean", "--all", "--dry-run"]);
    assert!(dry.contains("Would remove 5 things"), "{dry}");
    assert!(
        env.ok(&["storage", "clean", "--older-than", "1h", "--yes"])
            .contains("Nothing was removed.")
    );
    assert_eq!(env.ok(&["storage"]), listed);

    // By kind, by ID, and the rest.
    env.ok(&["storage", "clean", "--logs", "--yes"]);
    assert!(env.ok(&["log", "list"]).contains("No log is kept."));
    let id = env
        .ok(&["inbox", "list"])
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    env.ok(&["storage", "clean", "--inbox", &id, "--yes"]);
    assert!(env.ok(&["inbox", "list"]).contains("Inbox is empty."));
    assert_eq!(
        env.ok(&["storage"]).matches("removed in 14 days").count(),
        3
    );
    env.ok(&["storage", "clean", "--all", "--yes"]);
    assert!(
        env.ok(&["storage"])
            .starts_with("QRSend keeps nothing on this machine.")
    );
    assert!(
        env.ok(&["cache", "list"])
            .contains("No cached send sessions.")
    );
}

#[test]
fn nothing_sent_is_kept_when_asked_not_to() {
    let env = Env::new("nokeep");
    fs::write(env.path("a.bin"), noise(50_000)).unwrap();
    let sent = |env: &Env| env.ok(&["storage"]).matches("removed in 14 days").count();

    // Kept, unless told otherwise.
    env.ok(&["send", "--plain", "a.bin", "--export-frames", "f"]);
    assert_eq!(sent(&env), 1);

    // The option: gone when sending ends, and what was received is whole.
    let out = env.run(&[
        "send",
        "--plain",
        "a.bin",
        "--export-frames",
        "g",
        "--no-keep",
    ]);
    assert!(out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("Not kept (--no-keep)"), "{said}");
    assert!(!said.contains("To resend later"), "{said}");
    assert_eq!(sent(&env), 1);
    env.ok(&["recv", "--images", "g", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/a.bin")).unwrap(), noise(50_000));

    // The same as a rule, from the environment — where "0" means no.
    for (value, kept) in [("1", 1), ("0", 2)] {
        let out = env
            .command("me", &["send", "--plain", "a.bin", "--export-frames", "h"])
            .env("QRSEND_NO_KEEP", value)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(sent(&env), kept, "QRSEND_NO_KEEP={value}");
    }

    // A send that fails after packing leaves nothing either.
    let before = sent(&env);
    let out = env.run(&[
        "send",
        "--plain",
        "a.bin",
        "--export-frames",
        "i",
        "--size",
        "64x64",
        "--no-keep",
    ]);
    assert!(!out.status.success());
    assert_eq!(sent(&env), before);
    assert!(!env.ok(&["storage"]).contains("left over"));

    // Sending a kept transfer again with the option is the last time.
    let id = env
        .ok(&["cache", "list"])
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    env.ok(&[
        "send",
        "--session",
        &id,
        "--export-frames",
        "j",
        "--no-keep",
    ]);
    assert!(!env.ok(&["cache", "list"]).contains(&id));

    // A sender that is killed removes nothing. What it was not to keep says
    // so, and the next send removes it — but not that of a sender still at it.
    #[cfg(unix)]
    {
        env.ok(&["send", "--plain", "a.bin", "--export-frames", "k"]);
        env.ok(&["send", "--plain", "a.bin", "--export-frames", "l"]);
        let ids: Vec<String> = env
            .ok(&["cache", "list"])
            .lines()
            .filter_map(|l| l.split_whitespace().next().map(String::from))
            .collect();
        let (killed, at_it) = (&ids[ids.len() - 2], &ids[ids.len() - 1]);
        let mut gone = Command::new(env!("CARGO_BIN_EXE_qrsend"))
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let gone_id = gone.id();
        gone.wait().unwrap();
        let mark = |id: &str, process: u32| {
            let file = env.path(&format!("me/cache/send/{id}/not-kept"));
            fs::write(file, process.to_string()).unwrap();
        };
        mark(killed, gone_id);
        mark(at_it, std::process::id());
        let listed = env.ok(&["storage"]);
        assert_eq!(
            listed
                .matches("not kept: removed when its send ends")
                .count(),
            2,
            "{listed}"
        );
        env.ok(&["send", "--plain", "--text", "x", "--export-frames", "m"]);
        let list = env.ok(&["cache", "list"]);
        assert!(
            !list.contains(killed.as_str()) && list.contains(at_it.as_str()),
            "{list}"
        );
    }
}

/// A log is this user's alone, and showing one puts nothing but text on the
/// terminal, whatever is in the file and whoever could have put it there.
#[cfg(unix)]
#[test]
fn logs_are_private_and_shown_as_text_only() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let env = Env::new("logsafe");
    assert!(!env.run(&["recv", "--images", "nowhere"]).status.success());
    let dir = env.path("me/cache/logs");
    assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
    let log = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::metadata(&log).unwrap().mode() & 0o777, 0o600);

    // Whatever got into the file: what is shown is text.
    let mut content = fs::read(&log).unwrap();
    content.extend_from_slice(b"    1.00 link   \x1b]0;owned\x07 \xff\n");
    fs::write(&log, content).unwrap();
    let shown = env.run(&["log"]);
    assert!(shown.status.success());
    let shown = String::from_utf8(shown.stdout).unwrap();
    assert!(
        !shown.contains('\u{1b}') && !shown.contains('\u{7}'),
        "{shown:?}"
    );
    assert!(shown.contains("\\u{1b}]0;owned\\u{7}"), "{shown:?}");
    assert!(shown.contains("end    stopped with an error"), "{shown:?}");

    // A link is no log, whatever its name, and neither is a file with a name
    // this program does not give.
    fs::write(env.path("secret"), "a secret").unwrap();
    std::os::unix::fs::symlink(env.path("secret"), dir.join("29991231T235959Z-recv-1.log"))
        .unwrap();
    fs::write(dir.join("29991231T235959Z-re cv-1.log"), "not a log").unwrap();
    fs::write(dir.join("notes.log"), "not a log").unwrap();
    let list = env.ok(&["log", "list"]);
    assert_eq!(list.matches(" UTC  ").count(), 1, "{list}");
    let shown = env.ok(&["log"]);
    assert!(
        !shown.contains("a secret") && !shown.contains("not a log"),
        "{shown}"
    );

    // From a directory that others could have filled, nothing is shown…
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    for args in [&["log"][..], &["log", "list"]] {
        let out = env.run(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not used for private data"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // … and no log is kept there, which holds no transfer up.
    let before = fs::read_dir(&dir).unwrap().count();
    env.ok(&["send", "--plain", "--text", "x", "--export-frames", "f"]);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), before);
    assert_eq!(env.ok(&["recv", "--images", "f"]).trim_end(), "x");
}

#[test]
fn only_the_last_logs_are_kept() {
    let env = Env::new("logs");
    for _ in 0..12 {
        // (A run that fails at once still leaves its log.)
        assert!(!env.run(&["recv", "--images", "nowhere"]).status.success());
    }
    let logs = fs::read_dir(env.path("me/cache/logs")).unwrap().count();
    assert_eq!(logs, 10);
    assert_eq!(env.ok(&["log", "list"]).matches(" UTC  ").count(), 10);
    assert!(env.ok(&["log", "clear"]).contains("Removed 10 log(s)."));
    assert!(!env.path("me/cache/logs").exists());
}

/// Two-way over a channel that carries bytes: the receiver says what it
/// still lacks, and the sender, which would otherwise write frames without
/// end, stops when the receiver has everything.
#[test]
fn two_way_over_a_lossy_byte_channel() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let env = Env::new("two-way");
    fs::write(env.path("data.bin"), noise(400_000)).unwrap();
    let mut sender = env
        .command(
            "me",
            &[
                "send",
                "--plain",
                "data.bin",
                "--seg-shift",
                "16",
                "--export-text",
                "-",
                "--feedback-from",
                "-",
            ],
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut receiver = env
        .command(
            "you",
            &["recv", "--text", "-", "--feedback-to", "-", "-o", "out"],
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The way there loses every third line; the way back loses nothing.
    let (frames, mut to_receiver) = (
        sender.stdout.take().unwrap(),
        receiver.stdin.take().unwrap(),
    );
    let there = std::thread::spawn(move || {
        for (i, line) in BufReader::new(frames)
            .lines()
            .map_while(|l| l.ok())
            .enumerate()
        {
            if i % 3 != 1 && writeln!(to_receiver, "{line}").is_err() {
                break;
            }
        }
    });
    let (reports, mut to_sender) = (
        receiver.stdout.take().unwrap(),
        sender.stdin.take().unwrap(),
    );
    let back = std::thread::spawn(move || {
        let mut last = String::new();
        for line in BufReader::new(reports).lines().map_while(|l| l.ok()) {
            let _ = writeln!(to_sender, "{line}");
            last = line;
        }
        last
    });
    let sent = sender.wait_with_output().unwrap();
    let received = receiver.wait_with_output().unwrap();
    there.join().unwrap();
    let last = back.join().unwrap();
    let said = String::from_utf8_lossy(&sent.stderr).into_owned();
    assert!(
        received.status.success(),
        "{}",
        String::from_utf8_lossy(&received.stderr)
    );
    assert!(sent.status.success(), "{said}");
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(400_000));
    assert!(last.starts_with("QSF1-"), "{last}");
    assert!(said.contains("The receiver has everything"), "{said}");
    // No number of frames was asked for: it stopped because it was told to,
    // and before it had buried the receiver under passes on end (with a
    // third lost, one and a half times the data gets there; a sender that
    // did not wait for the receiver's word wrote eight times as much).
    let wrote: u64 = said
        .lines()
        .find_map(|l| l.strip_prefix("Wrote ")?.split(' ').next()?.parse().ok())
        .unwrap();
    let needed = 400_000 / 1024;
    assert!(wrote >= needed, "{wrote}");
    assert!(wrote < needed * 5, "{wrote}");
}

/// Color codes: three codes in every place, a third of the pictures.
#[test]
fn color_codes_are_written_and_read_back() {
    let env = Env::new("color");
    fs::write(env.path("data.bin"), noise(120_000)).unwrap();
    let count = |dir: &str, more: &[&str]| {
        let args: Vec<&str> = [
            "send",
            "--plain",
            "data.bin",
            "--grid",
            "2",
            "--export-frames",
            dir,
        ]
        .iter()
        .chain(more)
        .copied()
        .collect();
        env.ok(&args);
        frames(&env.path(dir)).len()
    };
    let (plain, color) = (count("bw", &[]), count("rgb", &["--color"]));
    assert!(
        color * 3 <= plain + 2 && color * 3 + 2 >= plain,
        "{color} for {plain}"
    );
    let out = env.run(&["recv", "--images", "rgb", "-o", "out"]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("Color codes: reading red, green and blue apart"),
        "{said}"
    );
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(120_000));

    // Where they do not apply, the options say so.
    for (args, why) in [
        (
            &["--color", "--export-video", "v.y4m"][..],
            "--color applies to",
        ),
        (
            &["--color", "--export-text", "t.txt"][..],
            "--color applies to",
        ),
        (
            &["--auto-speed", "--export-frames", "x"][..],
            "--auto-speed applies to a window",
        ),
    ] {
        let args: Vec<&str> = ["send", "--plain", "--text", "x"]
            .iter()
            .chain(args)
            .copied()
            .collect();
        let out = env.run(&args);
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !out.status.success() && said.contains(why),
            "{args:?}: {said}"
        );
    }
}

/// A way back that brings nothing must not mean writing without end.
#[test]
fn without_an_answer_writing_stops_by_itself() {
    let env = Env::new("unanswered");
    fs::write(env.path("data.bin"), noise(50_000)).unwrap();
    fs::write(env.path("nobody.txt"), "").unwrap();
    let out = env.run(&[
        "send",
        "--plain",
        "data.bin",
        "--export-text",
        "frames.txt",
        "--feedback-from",
        "nobody.txt",
    ]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("No feedback came from the receiver"),
        "{said}"
    );
    // Three passes of some fifty frames each, and what it takes is all there.
    let lines = fs::read_to_string(env.path("frames.txt"))
        .unwrap()
        .lines()
        .count();
    assert!((100..400).contains(&lines), "{lines}");
    env.ok(&["recv", "--text", "frames.txt", "-o", "out"]);
    assert_eq!(fs::read(env.path("out/data.bin")).unwrap(), noise(50_000));
}

/// `--secure`: encrypted, between trusted devices, one way, nothing left.
#[test]
fn secure_takes_and_sends_only_what_is_encrypted_between_trusted_devices() {
    let env = Env::new("secure");
    let (alice, bob) = (device_id(&env, "alice"), device_id(&env, "bob"));
    let dave = device_id(&env, "dave");
    env.ok_as("alice", &["devices", "add", &bob, "--yes"]);
    env.ok_as("bob", &["devices", "add", &alice, "--yes"]);
    env.ok_as("dave", &["devices", "add", &bob, "--yes"]);

    // Sending: whatever opens a way in or leaves something behind is refused.
    let refused = |device: &str, args: &[&str], why: &str| {
        let out = env.run_as(device, args);
        let said = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !out.status.success() && said.contains(why),
            "{args:?}: {said}"
        );
    };
    let send = |more: &[&str]| -> Vec<String> {
        ["send", "--secure", "--text", "x", "--export-frames", "no"]
            .iter()
            .chain(more)
            .map(|s| s.to_string())
            .collect()
    };
    for (more, why) in [
        (&["--plain"][..], "encrypted transfers only"),
        (&[][..], "encrypted transfers only"),
        (&["--to", "bob", "--lan"][..], "nothing through the network"),
        (
            &["--to", "bob", "--two-way"][..],
            "takes nothing in from the receiver",
        ),
        (
            &["--to", "bob", "--feedback-from", "x"][..],
            "takes nothing in from the receiver",
        ),
        (&["--to", "bob", "--log", "log.txt"][..], "keeps no log"),
        // An ID in full is no way round the list of trusted devices.
        (&["--to", dave.as_str()][..], "trusted devices only"),
    ] {
        let args = send(more);
        refused(
            "alice",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            why,
        );
    }
    let out = env.run_as(
        "alice",
        &[
            "send",
            "--secure",
            "--to",
            &bob,
            "--text",
            "for bob",
            "--export-frames",
            "f",
        ],
    );
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success() && said.contains("Not kept (--secure)"),
        "{said}"
    );
    // Neither the packed transfer nor a log stays behind.
    assert!(
        env.ok_as("alice", &["cache", "list"])
            .contains("No cached send sessions")
    );
    assert!(!env.path("alice/cache/logs").exists());

    // Receiving: from a trusted device, encrypted for this one; and no log.
    assert_eq!(
        env.ok_as("bob", &["recv", "--secure", "--images", "f"])
            .trim_end(),
        "for bob"
    );
    assert!(!env.path("bob/cache/logs").exists());
    for (option, why) in [
        (&["--keep"][..], "keeps nothing in the inbox"),
        (&["--copy"][..], "clipboard"),
        (
            &["--feedback-to", "back.txt"][..],
            "tells the sender nothing",
        ),
        (&["--two-way"][..], "tells the sender nothing"),
        (&["--log", "log.txt"][..], "keeps no log"),
    ] {
        let args: Vec<&str> = ["recv", "--secure", "--images", "f"]
            .iter()
            .chain(option)
            .copied()
            .collect();
        refused("bob", &args, why);
    }
    // A transfer that stops halfway leaves nothing behind either: without
    // the option what arrived is kept and can be added to, with it not.
    fs::write(env.path("more.bin"), noise(300_000)).unwrap();
    env.ok_as(
        "alice",
        &[
            "send",
            "more.bin",
            "--to",
            "bob",
            "--seg-shift",
            "16",
            "--export-frames",
            "h",
        ],
    );
    fs::create_dir_all(env.path("half")).unwrap();
    for (i, frame) in frames(&env.path("h")).iter().enumerate() {
        if i % 2 == 0 {
            fs::copy(frame, env.path("half").join(frame.file_name().unwrap())).unwrap();
        }
    }
    let out = env.run_as("bob", &["recv", "--images", "half", "-o", "o"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--resume"));
    assert!(env.ok_as("bob", &["inbox", "list"]).contains("more.bin"));
    let out = env.run_as("bob", &["recv", "--secure", "--images", "half", "-o", "o"]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "{said}");
    assert!(
        said.contains("Not kept (--secure)") && !said.contains("--resume"),
        "{said}"
    );
    assert!(!env.ok_as("bob", &["inbox", "list"]).contains("more.bin"));
    // Encrypted for bob, but by a device bob does not trust.
    env.ok_as(
        "dave",
        &[
            "send",
            "--to",
            "bob",
            "--text",
            "from dave",
            "--export-frames",
            "d",
        ],
    );
    refused(
        "bob",
        &["recv", "--secure", "--images", "d"],
        "its sender is not a trusted device",
    );
    // Nothing of it stays behind, and without the option it is taken as ever.
    assert!(!env.ok_as("bob", &["inbox", "list"]).contains("text"));
    assert_eq!(
        env.ok_as("bob", &["recv", "--images", "d"]).trim_end(),
        "from dave"
    );
    // Not encrypted at all; the environment's word counts as the option does.
    env.ok_as(
        "alice",
        &[
            "send",
            "--plain",
            "--text",
            "for anyone",
            "--export-frames",
            "p",
        ],
    );
    let out = env
        .command("bob", &["recv", "--images", "p"])
        .env("QRSEND_SECURE", "1")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success() && said.contains("it is not encrypted"),
        "{said}"
    );
}
