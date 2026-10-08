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
  await page.getByPlaceholder("qrsend-id:1:age1…").fill(ids.cli);
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
    await page.goto("./#/preview");
    await page.getByRole("checkbox", { name: /Local network boost/ }).check();
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
      await page.getByRole("button", { name: "Copy log" }).click().catch(() => {});
      const log = await page.getByTestId("log-text").inputValue().catch(() => "no log");
      console.log(log.split("\n").filter((l) => / link /.test(l)).join("\n"));
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
  // A few pictures are enough to notice (and too few to finish, so the
  // receiver is still there to say so).
  const few = path.join(WORK, "web-color-few");
  fs.rmSync(few, { recursive: true, force: true });
  fs.mkdirSync(few);
  for (const f of fs.readdirSync(dir).sort().slice(0, 6)) fs.copyFileSync(path.join(dir, f), path.join(few, f));
  const start = cliBoth(["recv", "--images", few], "cli");
  expect(start.status).toBe(2);
  expect(start.stderr).toContain("Color codes: reading red, green and blue apart");
  const received = cliBoth(["recv", "--images", dir, "--stdout"], "cli");
  expect(received.stdout).toContain(long);
});
