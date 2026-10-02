// Renders QR stream videos with the CLI; the browser then sees them as its
// camera (Chrome's --use-file-for-fake-video-capture).
import fs from "node:fs";
import path from "node:path";
import { FAKE_TEXT, WORK, cli, filesVideo, hasCli, textVideo } from "./fixtures";

export default function globalSetup() {
  fs.rmSync(WORK, { recursive: true, force: true });
  fs.mkdirSync(path.join(WORK, "in/notes"), { recursive: true });
  if (!hasCli) return;
  cli(["send", "--plain", "--text", FAKE_TEXT, "--density", "low", "--export-y4m", textVideo, "--frames", "12", "--scale", "4"]);
  fs.writeFileSync(path.join(WORK, "in/notes/a.txt"), "alpha\n".repeat(50));
  fs.writeFileSync(path.join(WORK, "in/notes/b.md"), "# beta\n");
  cli(["send", "--plain", "in/notes", "--density", "low", "--export-y4m", filesVideo, "--frames", "20", "--scale", "4"]);
}
