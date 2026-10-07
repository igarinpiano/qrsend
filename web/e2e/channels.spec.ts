import { expect, test } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { FAKE_TEXT, WORK, textVideo } from "./fixtures";
import { capturePlayer, enablePreview, fakeCamera, writeY4m } from "./video";

// Other ways for the codes to travel than black-and-white through a camera.

test("colour codes: three codes per cell, recognised by the receiver on its own", async ({ playwright, baseURL }) => {
  fs.mkdirSync(WORK, { recursive: true });
  const video = path.join(WORK, "colour.y4m");
  const data = Buffer.from(Array.from({ length: 6000 }, (_, i) => (i * 7919) % 251));

  const senderBrowser = await playwright.chromium.launch();
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    const sender = await (await senderBrowser.newContext({ baseURL, viewport: { width: 1000, height: 700 } })).newPage();
    await enablePreview(sender, /Colour codes/);
    await sender.goto("./#/send");
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "colour.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("2");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByText("×3 colours")).toBeVisible();
    const frames = await capturePlayer(sender, 30);
    expect(frames.length).toBeGreaterThan(10);
    writeY4m(video, frames);

    receiverBrowser = await playwright.chromium.launch({ args: fakeCamera(video) });
    const context = await receiverBrowser.newContext({ baseURL, permissions: ["camera"], acceptDownloads: true });
    const receiver = await context.newPage();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("received-file")).toHaveText(["colour.bin"], { timeout: 90_000 });
    const [download] = await Promise.all([
      receiver.waitForEvent("download"),
      receiver.getByRole("listitem").filter({ hasText: "colour.bin" }).getByRole("button", { name: "Save" }).click(),
    ]);
    expect(fs.readFileSync(await download.path()).equals(data)).toBe(true);
  } finally {
    await receiverBrowser?.close();
    await senderBrowser.close();
  }
});

// A headless browser has no screen to share, so the test stands in for the
// browser's picker: "the screen" is a stream showing the sender's codes, and
// there is no camera at all. Everything of ours is real: the button, the
// captured stream and reading the codes from it.
test("screen capture: the receiver reads a shared screen instead of a camera", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({ args: fakeCamera(textVideo) });
  try {
    const context = await browser.newContext({ baseURL });
    await context.addInitScript(() => {
      const devices = navigator.mediaDevices;
      const open = devices.getUserMedia.bind(devices);
      devices.getUserMedia = () => Promise.reject(new DOMException("no camera here", "NotFoundError"));
      devices.getDisplayMedia = () => open({ video: true });
    });
    const receiver = await context.newPage();
    await enablePreview(receiver, /Receive from the screen/);
    await receiver.goto("./#/receive");
    await expect(receiver.getByText("Cannot open the camera")).toBeVisible();
    await receiver.getByRole("button", { name: "Use the screen" }).click();
    await expect(receiver.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
  } finally {
    await browser.close();
  }
});

test("screen capture is not offered unless the preview feature is on", async ({ page }) => {
  await page.goto("./#/receive");
  await expect(page.getByText("Use a video file")).toBeVisible();
  await expect(page.getByRole("button", { name: "Use the screen" })).toHaveCount(0);
});
