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

// The two are on one machine here. On a Mac they meet at its loopback
// address (the firewall cuts connections to an unsigned program at any
// other), which a browser leaves out unless told otherwise. Elsewhere the
// machine's own address on the network does, as between two machines.
const loopback = ["--allow-loopback-in-peer-connection"];
const address = process.platform === "darwin" ? ["--lan-address", "127.0.0.1"] : [];

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
    ["send", "--plain", "big.bin", "--lan", ...address, "--export-video", video, "--frames", "40", "--fps", "8"],
    { cwd: dir, env: { ...env("cli-link"), QRSEND_TRACE: "1" } },
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
      const since = Date.now();
      await expect(page.getByTestId("received-file"))
        .toHaveText(["big.bin"], { timeout: 90_000 })
        .catch(async (e) => {
          // What each side made of the attempt.
          await page.getByRole("button", { name: "Copy log" }).click().catch(() => {});
          const log = await page.getByTestId("log-text").inputValue().catch(() => "no log");
          console.log(log.split("\n").filter((l) => / link | rx +(open offer|connected|step)/.test(l)).join("\n"));
          console.log("the sender said:\n" + said.split("\n").filter((l) => !/ B from |BufferedAmount|data: /.test(l)).slice(0, 60).join("\n"));
          throw e;
        });
      if (process.env.QRSEND_LINK_MB) console.log(`TOOK ${megabytes} MiB in ${(Date.now() - since) / 1000} s (from opening the page)`);
      // The log says between which kinds of addresses the connection was
      // made (which tells a local network from the internet), and no more.
      await page.getByRole("button", { name: "Copy log" }).click();
      const log = await page.getByTestId("log-text").inputValue();
      expect(log).toMatch(/answer: path chosen {2}own="v4 (loopback|private)[^"]*" other="v4 (loopback|private)[^"]*"/);
      expect(log).not.toContain("127.0.0.1");
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
