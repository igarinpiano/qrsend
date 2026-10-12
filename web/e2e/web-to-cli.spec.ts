import { expect, test, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { WORK, cli, cliBoth, hasCli, readIds } from "./fixtures";

test.skip(!hasCli, "qrsend CLI binary not built");

/** Saves frames drawn by the player as PNG files for the CLI to read. */
async function captureFrames(page: Page, dir: string, count: number): Promise<number> {
  await expect(page.getByLabel("QR code stream")).toBeVisible();
  const frames: string[] = await page.evaluate(async (wanted) => {
    const canvas = document.querySelector("canvas")!;
    const out: string[] = [];
    let last = canvas.dataset.frames;
    const until = Date.now() + 30_000;
    while (out.length < wanted && Date.now() < until) {
      await new Promise((r) => setTimeout(r, 20));
      if (canvas.dataset.frames !== last) {
        last = canvas.dataset.frames;
        out.push(canvas.toDataURL("image/png"));
      }
    }
    return out;
  }, count);
  fs.mkdirSync(dir, { recursive: true });
  frames.forEach((url, i) =>
    fs.writeFileSync(path.join(dir, `f${String(i).padStart(3, "0")}.png`), Buffer.from(url.split(",")[1], "base64")),
  );
  return frames.length;
}

test("web → CLI: frames rendered by the browser decode in the CLI", async ({ page }) => {
  const message = "Sent from the browser 🌐";
  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill(message);
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByLabel("Density").selectOption("low");
  await page.getByRole("button", { name: "Start sending" }).click();

  const dir = path.join(WORK, "web-frames");
  expect(await captureFrames(page, dir, 12)).toBeGreaterThan(4);
  expect(cli(["recv", "--images", dir])).toContain(message);
});

test("web → CLI: files in a 3×3 grid with automatic code size", async ({ page }) => {
  const data = Buffer.from("grid ".repeat(4000));
  await page.goto("./#/send");
  await page.locator('input[type="file"]').first().setInputFiles({ name: "grid.txt", mimeType: "text/plain", buffer: data });
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByLabel("Codes on screen").selectOption("3");
  await page.getByRole("button", { name: "Start sending" }).click();

  const dir = path.join(WORK, "web-grid");
  expect(await captureFrames(page, dir, 6)).toBeGreaterThan(1);
  const out = path.join(WORK, "web-grid-out");
  cli(["recv", "--images", dir, "-o", out]);
  expect(fs.readFileSync(path.join(out, "grid.txt")).equals(data)).toBe(true);
});

test("web → CLI: encrypted for a paired device and signed by the browser's key", async ({ page }) => {
  const ids = readIds();
  const message = "Browser to CLI, encrypted 🔒";
  // The browser creates its own identity (non-extractable keys) and pairs both ways.
  await page.goto("./#/devices");
  await page.getByRole("button", { name: "Create device ID" }).click();
  await page.getByText("Show ID as text").click();
  const browserId = (await page.getByTestId("my-id").textContent())!;
  await page.getByPlaceholder("qrsend-id:…").fill(ids.cli);
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Yes, trust it" }).click();
  cli(["devices", "add", browserId, "--name", "browser", "--yes"], "cli");

  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill(message);
  await page.getByRole("checkbox", { name: /cli/ }).check();
  await page.getByRole("button", { name: "Start sending" }).click();

  const dir = path.join(WORK, "web-encrypted");
  expect(await captureFrames(page, dir, 16)).toBeGreaterThan(4);
  const received = cliBoth(["recv", "--images", dir], "cli");
  expect(received.stdout).toContain(message);
  expect(received.stderr).toContain("browser ✓");
  expect(received.stderr).toContain("Encrypted for this device");
  // A device it was not addressed to cannot read it.
  const other = cliBoth(["recv", "--images", dir], "webdev");
  expect(other.status).not.toBe(0);
  expect(other.stderr).toContain("encrypted for another device");
});

// Forward secrecy: the command line shows a one-time key before it receives,
// the browser encrypts to it as well, and the command line destroys the key
// once the transfer has arrived. The same pictures, kept, open no more.
test("web → CLI: forward secrecy with the receiver's one-time key", async ({ page }) => {
  const message = "Browser to CLI, and gone afterwards";
  const device = "fs-cli";
  const id = cli(["id", "--name", device, "--no-qr"], device)
    .split("\n")
    .find((l) => l.startsWith("ID:"))!
    .slice(3)
    .trim();
  await page.goto("./#/devices");
  await page.getByRole("button", { name: "Create device ID" }).click();
  await page.getByText("Show ID as text").click();
  const browserId = (await page.getByTestId("my-id").textContent())!;
  await page.getByPlaceholder("qrsend-id:…").fill(id);
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Yes, trust it" }).click();
  cli(["devices", "add", browserId, "--name", "browser", "--yes"], device);
  // (A second device, only to be chosen beside the first for a moment.)
  const other = "fs-other";
  const otherId = cli(["id", "--name", other, "--no-qr"], other)
    .split("\n")
    .find((l) => l.startsWith("ID:"))!
    .slice(3)
    .trim();
  await page.getByPlaceholder("qrsend-id:…").fill(otherId);
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Yes, trust it" }).click();

  // The receiver makes its key when it starts to receive (here from nothing).
  const nothing = path.join(WORK, "fs-nothing.txt");
  fs.writeFileSync(nothing, "");
  const shown = cliBoth(["recv", "--one-time-key", "--text", nothing], device);
  const key = shown.stderr.split("\n").find((l) => l.startsWith("QSK1-"))!;
  expect(key.length).toBeGreaterThan(1500);

  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill(message);
  await page.getByRole("checkbox", { name: new RegExp(device) }).check();
  // A key that is not that device's is not taken.
  await page.getByTestId("one-time-input").fill(key.slice(0, -6) + "AAAAAA");
  await page.getByRole("button", { name: "Use this key" }).click();
  await expect(page.getByText("belongs to none of the chosen devices")).toBeVisible();
  await page.getByTestId("one-time-input").fill(key);
  await page.getByRole("button", { name: "Use this key" }).click();
  await expect(page.getByTestId("one-time-taken")).toBeVisible();
  // With the only receiver's key read there is nothing left to scan, and
  // the page says so instead of offering to scan again (which read as "one
  // more is wanted"). Reading a key again is still possible, on request.
  const scan = page.getByRole("button", { name: /Scan the (next )?receiver's one-time key/ });
  await expect(page.getByTestId("one-time-state")).toContainText("nothing more to scan");
  // A second receiver wants a key of its own: the page names who is still
  // without one, and offers to scan the next.
  await page.getByRole("checkbox", { name: new RegExp(other) }).check();
  await expect(page.getByTestId("one-time-state")).toContainText(`Still without a one-time key: ${other}`);
  await expect(page.getByRole("button", { name: "Scan the next receiver's one-time key" })).toBeVisible();
  await page.getByRole("checkbox", { name: new RegExp(other) }).uncheck();
  await expect(page.getByTestId("one-time-state")).toContainText("nothing more to scan");
  await expect(scan).toHaveCount(0);
  await expect(page.getByTestId("one-time-input")).toHaveCount(0);
  await page.getByTestId("one-time-again").click();
  await expect(scan).toBeVisible();
  await page.getByRole("button", { name: "Start sending" }).click();
  await expect(page.getByText("encrypted (forward secrecy, post-quantum)")).toBeVisible();

  const dir = path.join(WORK, "web-forward");
  expect(await captureFrames(page, dir, 24)).toBeGreaterThan(6);
  const received = cliBoth(["recv", "--images", dir], device);
  expect(received.stdout).toContain(message);
  expect(received.stderr).toContain("with forward secrecy");
  expect(received.stderr).toContain("The one-time key of this transfer is destroyed");
  // Read again from the same pictures: the key is gone, and with it the way in.
  const again = cliBoth(["recv", "--images", dir], device);
  expect(again.status).not.toBe(0);
  expect(again.stderr).toContain("encrypted for another device");
});

// A sending browser also makes an offer that needs no answer shown to its
// camera: the receiver's certificate follows from a seed in the offer
// (PROTOCOL §12.5). The CLI reads it off the pictures and connects to the
// page, which is still there, sending.
test("web → CLI: the CLI connects to the sending browser over the network", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    // (Both are on this machine: they meet at its loopback address, which a
    // browser leaves out unless told otherwise.)
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", "--allow-loopback-in-peer-connection"],
  });
  try {
    const page = await (await browser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    // What the page logs about the connection, to print if it fails.
    const said: string[] = [];
    await page.addInitScript(() => localStorage.setItem("qrsend.debug.log", "1"));
    page.on("console", (m) => {
      if (m.text().startsWith("[qrsend]") && / link | tx +(a connection|connection)/.test(m.text())) said.push(m.text());
    });
    await page.goto("./#/preview");
    await page.getByRole("combobox", { name: /Local network boost/ }).selectOption("wifi");
    await page.goto("./#/send");
    await page.getByRole("tab", { name: "Text" }).click();
    // Far more than the pictures taken below can carry.
    const long = Array.from({ length: 60_000 }, () => Math.random().toString(36).slice(2)).join(" ");
    await page.getByPlaceholder("Paste or type anything…").fill(long);
    await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await page.getByRole("button", { name: "Start sending" }).click();
    await expect(page.getByRole("button", { name: /offering LAN/ })).toBeVisible();
    for (let i = 0; i < 4; i++) await page.getByRole("button", { name: "Slower" }).click();
    const dir = path.join(WORK, "web-offer");
    fs.rmSync(dir, { recursive: true, force: true });
    expect(await captureFrames(page, dir, 60)).toBeGreaterThan(30);
    const received = cliBoth(["recv", "--images", dir, "--stdout"], "cli", { QRSEND_TRACE: "1" });
    if (!received.stderr.includes("Receiving over the network")) {
      // What each side made of the attempt.
      console.log(received.stderr.split("\n").filter((l) => !/ B from |BufferedAmount|data: /.test(l)).slice(0, 60).join("\n"));
      // (What the page said while the CLI ran arrives only now.)
      await page.waitForTimeout(1000);
      console.log(said.join("\n"));
    }
    expect(received.stderr).toContain("Receiving over the network");
    expect(received.status).toBe(0);
    expect(received.stdout).toContain(long);
  } finally {
    await browser.close();
  }
});

// Color codes stack three codes in red, green and blue. Nothing tells the
// receiver: the CLI notices that the colors carry different codes.
test("web → CLI: color codes are told apart", async ({ page }) => {
  await page.goto("./#/preview");
  await page.getByRole("checkbox", { name: /Color codes/ }).check();
  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  const long = Array.from({ length: 1500 }, () => Math.random().toString(36).slice(2)).join(" ");
  await page.getByPlaceholder("Paste or type anything…").fill(long);
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByRole("button", { name: "Start sending" }).click();
  for (let i = 0; i < 4; i++) await page.getByRole("button", { name: "Slower" }).click();
  const dir = path.join(WORK, "web-color");
  fs.rmSync(dir, { recursive: true, force: true });
  expect(await captureFrames(page, dir, 40)).toBeGreaterThan(20);
  // Two pictures are enough to notice (and too few to finish, so the
  // receiver is still there to say so). Every color of the first pictures
  // is read, not only what shows in black and white: six pictures, which
  // once left the transfer unfinished, now complete it.
  const few = path.join(WORK, "web-color-few");
  fs.rmSync(few, { recursive: true, force: true });
  fs.mkdirSync(few);
  for (const f of fs.readdirSync(dir).sort().slice(0, 2)) fs.copyFileSync(path.join(dir, f), path.join(few, f));
  const start = cliBoth(["recv", "--images", few], "cli");
  expect(start.status).toBe(2);
  expect(start.stderr).toContain("Color codes: reading red, green and blue apart");
  const received = cliBoth(["recv", "--images", dir, "--stdout"], "cli");
  expect(received.stdout).toContain(long);
});
