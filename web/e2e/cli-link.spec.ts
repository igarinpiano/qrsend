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

// The second receiver turns up when the sender has waited longer than a
// connection's handshake holds out (half a minute): the offer it reads, the
// one in the video, must still be good. (It was not: the connection was set
// up with the offer, gave up unanswered, and took the offer with it.)
const cases = [
  { title: "CLI → web: the browser connects to the sender, which is shown nothing", name: "cli-link", megabytes: 6, late: 0 },
  { title: "CLI → web: an offer nobody answered yet stays good", name: "cli-link-late", megabytes: 1, late: 36_000 },
];

for (const { title, name, megabytes: size, late } of cases) test(title, async ({ playwright, baseURL }) => {
  test.setTimeout(120_000 + late);
  const dir = path.join(WORK, name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  // Far more than the few pictures of the video can carry.
  const megabytes = Number(process.env.QRSEND_LINK_MB ?? size);
  const data = crypto.randomBytes(megabytes << 20);
  fs.writeFileSync(path.join(dir, "big.bin"), data);
  const video = path.join(dir, "codes.y4m");
  const sender = spawn(
    BIN,
    ["send", "--plain", "big.bin", "--lan", ...address, "--export-video", video, "--frames", "40", "--fps", "8"],
    { cwd: dir, env: { ...env(name), QRSEND_TRACE: "1" } },
  );
  let said = "";
  sender.stderr.on("data", (d) => (said += d));
  const ended = new Promise<number | null>((resolve) => sender.on("exit", resolve));
  try {
    // The video is written; the sender now waits on the network.
    await expect.poll(() => said, { timeout: 60_000 }).toContain("Waiting for the receiver on the network");
    await new Promise((resolve) => setTimeout(resolve, late));
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

// A sender that offers a connection and cannot be reached (its firewall
// turns the receiver away, as a rule) tells nobody: here it is stopped
// outright. The receiver is the one that can tell, and says so.
test("CLI → web: a sender that cannot be reached is said to be so", async ({ playwright, baseURL }) => {
  test.skip(process.platform === "win32", "stops a process with a signal");
  test.setTimeout(120_000);
  const name = "cli-link-unreached";
  const dir = path.join(WORK, name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "big.bin"), crypto.randomBytes(1 << 20));
  const video = path.join(dir, "codes.y4m");
  const sender = spawn(
    BIN,
    ["send", "--plain", "big.bin", "--lan", ...address, "--export-video", video, "--frames", "40", "--fps", "8"],
    { cwd: dir, env: env(name) },
  );
  let said = "";
  sender.stderr.on("data", (d) => (said += d));
  try {
    await expect.poll(() => said, { timeout: 60_000 }).toContain("Waiting for the receiver on the network");
    sender.kill("SIGSTOP");
    const browser = await playwright.chromium.launch({ args: [...fakeCamera(video), ...loopback] });
    try {
      const context = await browser.newContext({ baseURL, permissions: ["camera"] });
      const page = await context.newPage();
      await page.goto("./#/receive");
      await expect(page.getByTestId("open-offer-unreached")).toContainText("could not be reached", { timeout: 60_000 });
    } finally {
      await browser.close();
    }
  } finally {
    sender.kill("SIGKILL");
  }
});
