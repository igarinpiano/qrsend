// A network connection between the command-line program and a browser
// (WebRTC, PROTOCOL §12.4).
import { expect, test } from "@playwright/test";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { BIN, WORK, env, hasCli } from "./fixtures";
import { fakeCamera } from "./video";

test.skip(!hasCli, "qrsend CLI binary not built");

// The two are on one machine here, so they meet at its loopback address,
// which a browser leaves out unless told otherwise.
const loopback = ["--allow-loopback-in-peer-connection"];

test("CLI → web: the browser connects to the sender, which is shown nothing", async ({ playwright, baseURL }) => {
  test.setTimeout(120_000);
  const dir = path.join(WORK, "cli-link");
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  // Far more than the few pictures of the video can carry.
  const megabytes = Number(process.env.QRSEND_LINK_MB ?? 6);
  const data = crypto.randomBytes(megabytes << 20);
  fs.writeFileSync(path.join(dir, "big.bin"), data);
  const video = path.join(dir, "codes.y4m");
  const sender = spawn(
    BIN,
    ["send", "--plain", "big.bin", "--lan", "--lan-address", "127.0.0.1", "--export-video", video, "--frames", "40", "--fps", "8"],
    { cwd: dir, env: env("cli-link") },
  );
  let said = "";
  sender.stderr.on("data", (d) => (said += d));
  const ended = new Promise<number | null>((resolve) => sender.on("exit", resolve));
  try {
    // The video is written; the sender now waits on the network.
    await expect.poll(() => said, { timeout: 60_000 }).toContain("Waiting for the receiver on the network");
    const browser = await playwright.chromium.launch({ args: [...fakeCamera(video), ...loopback] });
    try {
      const context = await browser.newContext({ baseURL, permissions: ["camera"], acceptDownloads: true });
      const page = await context.newPage();
      await page.goto("./#/receive");
      if (process.env.QRSEND_LINK_TRACE) {
        await page.waitForTimeout(20_000);
        await page.getByRole("button", { name: "Copy log" }).click().catch(() => {});
        console.log(await page.getByTestId("log-text").inputValue().catch(() => "no log"));
        console.log("SENDER SAID", said);
      }
      const since = Date.now();
      await expect(page.getByTestId("received-file")).toHaveText(["big.bin"], { timeout: 90_000 });
      if (process.env.QRSEND_LINK_MB) console.log(`TOOK ${megabytes} MiB in ${(Date.now() - since) / 1000} s (from opening the page)`);
      // The sender heard that everything arrived, and stops by itself.
      expect(await ended).toBe(0);
      expect(said).toContain("The receiver has everything");
    } finally {
      await browser.close();
    }
  } finally {
    sender.kill();
  }
});
