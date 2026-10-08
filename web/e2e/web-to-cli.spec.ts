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

// A browser offers its network connection to other browsers only (WebRTC);
// the command-line receiver cannot take it. It says so, instead of leaving
// the person to wonder why the network is not used, and reads the codes.
test("web → CLI: a browser's offer to connect is declined in so many words", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const page = await (await browser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await page.goto("./#/preview");
    await page.getByRole("checkbox", { name: /Local network boost/ }).check();
    await page.goto("./#/send");
    await page.getByRole("tab", { name: "Text" }).click();
    await page.getByPlaceholder("Paste or type anything…").fill("Over the codes, then");
    await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await page.getByRole("button", { name: "Start sending" }).click();
    await expect(page.getByRole("button", { name: /offering LAN/ })).toBeVisible();
    for (let i = 0; i < 4; i++) await page.getByRole("button", { name: "Slower" }).click();
    const dir = path.join(WORK, "web-offer");
    fs.rmSync(dir, { recursive: true, force: true });
    expect(await captureFrames(page, dir, 30)).toBeGreaterThan(12);
    const received = cliBoth(["recv", "--images", dir], "cli");
    expect(received.stdout).toContain("Over the codes, then");
    expect(received.stderr).toContain("offers a network connection, but only to another browser");
  } finally {
    await browser.close();
  }
});
