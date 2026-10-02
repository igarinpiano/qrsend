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
        Command::new(env!("CARGO_BIN_EXE_qrsend"))
            .args(args)
            .current_dir(&self.root)
            .env("QRSEND_DATA_DIR", self.path("data"))
            .env("QRSEND_CACHE_DIR", self.path("cache"))
            .output()
            .unwrap()
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
