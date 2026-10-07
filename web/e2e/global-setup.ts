// Renders QR stream videos with the CLI; the browser then sees them as its
// camera (Chrome's --use-file-for-fake-video-capture).
import fs from "node:fs";
import path from "node:path";
import {
  FAKE_TEXT,
  SECRET_TEXT,
  WORK,
  cli,
  encryptedVideo,
  filesVideo,
  hasCli,
  hasFfmpeg,
  idsFile,
  noise,
  segmentsVideo,
  textVideo,
  webmVideo,
} from "./fixtures";

const idOf = (device: string) =>
  cli(["id", "--name", device, "--no-qr"], device)
    .split("\n")
    .find((l) => l.startsWith("ID:"))!
    .slice(3)
    .trim();

export default function globalSetup() {
  fs.rmSync(WORK, { recursive: true, force: true });
  fs.mkdirSync(path.join(WORK, "in/notes"), { recursive: true });
  if (!hasCli) return;
  const small = ["--density", "low", "--scale", "4"];
  cli(["send", "--plain", "--text", FAKE_TEXT, ...small, "--export-video", textVideo, "--frames", "12"]);
  fs.writeFileSync(path.join(WORK, "in/notes/a.txt"), "alpha\n".repeat(50));
  fs.writeFileSync(path.join(WORK, "in/notes/b.md"), "# beta\n");
  cli(["send", "--plain", "in/notes", ...small, "--export-video", filesVideo, "--frames", "20"]);

  // Many small segments (4 KiB each) exercise random-access storage.
  fs.mkdirSync(path.join(WORK, "in/multi/sub"), { recursive: true });
  fs.writeFileSync(path.join(WORK, "in/multi/big.bin"), noise(40_000));
  fs.writeFileSync(path.join(WORK, "in/multi/sub/note.txt"), "nested\n");
  cli(["send", "--plain", "in/multi", "--seg-shift", "12", ...small, "--grid", "2", "--passes", "2", "--export-video", segmentsVideo]);

  // Two paired devices: "cli", and "webdev" whose identity the browser adopts.
  const ids = { cli: idOf("cli"), web: idOf("webdev"), webSecret: "" };
  ids.webSecret = fs.readFileSync(path.join(WORK, "webdev/config/identity"), "utf8");
  cli(["devices", "add", ids.web, "--yes"], "cli");
  fs.writeFileSync(idsFile, JSON.stringify(ids));
  cli(["send", "--to", "webdev", "--text", SECRET_TEXT, ...small, "--export-video", encryptedVideo, "--passes", "3"], "cli");

  if (hasFfmpeg) {
    cli(["send", "--plain", "--text", FAKE_TEXT, ...small, "--export-video", webmVideo, "--passes", "3", "--fps", "10"]);
  }
}
