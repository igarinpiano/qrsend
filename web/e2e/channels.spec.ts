import { expect, test } from "@playwright/test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { FAKE_TEXT, WORK, textVideo } from "./fixtures";
import { blank, captureImage, capturePlayer, enablePreview, fakeCamera, writeY4m } from "./video";

// Other ways for the codes to travel than black-and-white through a camera.

test("color codes: three codes per cell, recognized by the receiver on its own", async ({ playwright, baseURL }) => {
  fs.mkdirSync(WORK, { recursive: true });
  const video = path.join(WORK, "color.y4m");
  const data = Buffer.from(Array.from({ length: 6000 }, (_, i) => (i * 7919) % 251));

  const senderBrowser = await playwright.chromium.launch();
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    const sender = await (await senderBrowser.newContext({ baseURL, viewport: { width: 1000, height: 700 } })).newPage();
    await enablePreview(sender, /Color codes/);
    await sender.goto("./#/send");
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "color.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("2");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByText("×3 colors")).toBeVisible();
    const frames = await capturePlayer(sender, 30);
    expect(frames.length).toBeGreaterThan(10);
    writeY4m(video, frames);

    receiverBrowser = await playwright.chromium.launch({ args: fakeCamera(video) });
    const context = await receiverBrowser.newContext({ baseURL, permissions: ["camera"], acceptDownloads: true });
    const receiver = await context.newPage();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("received-file")).toHaveText(["color.bin"], { timeout: 90_000 });
    const [download] = await Promise.all([
      receiver.waitForEvent("download"),
      receiver.getByRole("listitem").filter({ hasText: "color.bin" }).getByRole("button", { name: "Save" }).click(),
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

// Two browsers on one machine reach each other over its network address, as
// two devices on a network would. The macOS firewall forbids that to programs
// it does not know, which includes the browser the tests bring along; the
// installed Google Chrome is let through, so a developer's Mac uses that one.
const lanBrowser = process.platform === "darwin" && !process.env.CI ? "chrome" : undefined;
const hasLanBrowser = !lanBrowser || fs.existsSync("/Applications/Google Chrome.app");

test("local network: after a handshake through the codes, the transfer travels over a direct connection", async ({ playwright, baseURL }) => {
  test.skip(!hasLanBrowser, "needs Google Chrome: the firewall keeps the bundled browser off the network");
  fs.mkdirSync(WORK, { recursive: true });
  const codesVideo = path.join(WORK, "lan-codes.y4m");
  const answerVideo = path.join(WORK, "lan-answer.y4m");
  writeY4m(answerVideo, [blank()]);
  // Far more than the few captured frames could carry.
  const data = crypto.randomBytes(Number(process.env.QRSEND_LAN_MB ?? 20) * 1_000_000);
  const dataFile = path.join(WORK, "lan.bin");
  fs.writeFileSync(dataFile, data);

  const senderBrowser = await playwright.chromium.launch({ channel: lanBrowser, args: fakeCamera(answerVideo) });
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    const sender = await (await senderBrowser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await enablePreview(sender, /Local network boost/, /Show measurements/);
    await sender.goto("./#/send");
    await sender.locator('input[type="file"]').first().setInputFiles(dataFile);
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    // Small codes: easy on the camera, which only has to carry the handshake.
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByRole("button", { name: "Start sending" }).click();
    // Its stream carries the offer to connect; nothing answers yet.
    await expect(sender.getByRole("button", { name: /offering LAN/ })).toBeVisible();
    // Slow enough that the capture below misses no frame: the offer is only in some of them.
    for (let i = 0; i < 4; i++) await sender.getByRole("button", { name: "Slower" }).click();
    const frames = await capturePlayer(sender, 40);
    expect(frames.length).toBeGreaterThan(20);
    writeY4m(codesVideo, frames);
    await sender.getByRole("button", { name: /Two-way/ }).click();

    // The receiver sees the offer and answers with a code, without being asked.
    receiverBrowser = await playwright.chromium.launch({ channel: lanBrowser, args: fakeCamera(codesVideo) });
    const context = await receiverBrowser.newContext({ baseURL, permissions: ["camera"], acceptDownloads: true });
    const receiver = await context.newPage();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("link-answer")).toBeVisible({ timeout: 30_000 });
    const answer = await captureImage(receiver, "link-answer");
    writeY4m(answerVideo, [answer, answer, answer]);

    // The sender's camera sees the answer: the two connect, and the file arrives that way.
    await sender.getByRole("button", { name: /Two-way/ }).click();
    await expect(receiver.getByTestId("link-connected")).toBeVisible({ timeout: 30_000 });
    const started = Date.now();
    await expect(sender.getByTestId("link-up")).toBeVisible();
    // The screen keeps sending as well, now from the other end of the transfer.
    const shown = Number(await sender.getByLabel("QR code stream").getAttribute("data-frames"));
    await expect.poll(async () => Number(await sender.getByLabel("QR code stream").getAttribute("data-frames"))).toBeGreaterThan(shown);
    if (process.env.QRSEND_LAN_TRACE) {
      await receiver.evaluate(() => localStorage.setItem("qrsend.preview.stats", "1"));
      for (let i = 0; i < 40 && !(await receiver.getByTestId("received-file").count()); i++) {
        console.log(((Date.now() - started) / 1000).toFixed(1), await sender.getByTestId("tx-stats").innerText().catch(() => ""));
        console.log("   ", await receiver.getByTestId("remaining").innerText().catch(() => ""));
        await new Promise((r) => setTimeout(r, 500));
      }
    }
    await expect(receiver.getByTestId("received-file")).toHaveText(["lan.bin"], { timeout: 90_000 });
    console.log(`local network: ${(data.length / 1024 / ((Date.now() - started) / 1000)).toFixed(0)} KiB/s`);
    console.log(await sender.getByTestId("tx-stats").innerText());
    await expect(sender.getByTestId("tx-steps")).toHaveText(/offered after [\d.]+ s · answer read after [\d.]+ s · connected\s+after [\d.]+ s/);
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
    const [download] = await Promise.all([
      receiver.waitForEvent("download"),
      receiver.getByRole("listitem").filter({ hasText: "lan.bin" }).getByRole("button", { name: "Save" }).click(),
    ]);
    expect(fs.readFileSync(await download.path()).equals(data)).toBe(true);
  } finally {
    await receiverBrowser?.close();
    await senderBrowser.close();
  }
});

test("measurements appear only with their preview feature", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({ args: fakeCamera(textVideo) });
  try {
    const page = await (await browser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
    await expect(page.getByTestId("scan-stats")).toHaveCount(0);
    await enablePreview(page, /Show measurements/);
    await page.goto("./#/receive");
    // How long the decoder takes per picture, with codes in it and without.
    await expect(page.getByTestId("scan-stats")).toHaveText(/\d+ reads\/s · \d+ ms with codes, \d+ ms without/, { timeout: 30_000 });
  } finally {
    await browser.close();
  }
});

// Two devices that trust each other connect once the usual way (the
// receiver's answer code is shown to the sender's camera). From then on the
// sender knows how to reach that receiver, and the receiver connects by
// itself: the sender's camera sees nothing at all the second time.
test("remembered devices: the second connection needs no answer code", async ({ playwright, baseURL }) => {
  test.skip(!hasLanBrowser, "needs Google Chrome: the firewall keeps the bundled browser off the network");
  test.setTimeout(240_000);
  fs.mkdirSync(WORK, { recursive: true });
  const codesVideo = path.join(WORK, "known-codes.y4m");
  const answerVideo = path.join(WORK, "known-answer.y4m");
  writeY4m(answerVideo, [blank()]);
  writeY4m(codesVideo, [blank()]);
  const first = path.join(WORK, "known-first.bin");
  const second = path.join(WORK, "known-second.bin");
  fs.writeFileSync(first, crypto.randomBytes(3_000_000));
  fs.writeFileSync(second, crypto.randomBytes(5_000_000));

  const senderBrowser = await playwright.chromium.launch({ channel: lanBrowser, args: fakeCamera(answerVideo) });
  const receiverBrowser = await playwright.chromium.launch({ channel: lanBrowser, args: fakeCamera(codesVideo) });
  try {
    const sender = await (await senderBrowser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    const receiver = await (await receiverBrowser.newContext({ baseURL, permissions: ["camera"] })).newPage();

    // Each gets an identity and trusts the other.
    const identify = async (page: typeof sender, name: string) => {
      await page.goto("./#/devices");
      await page.getByLabel("Device name").fill(name);
      await page.getByRole("button", { name: "Create device ID" }).click();
      await page.getByText("Show ID as text").click();
      return (await page.getByTestId("my-id").textContent())!;
    };
    const trust = async (page: typeof sender, id: string) => {
      await page.getByPlaceholder("qrsend-id:1:age1…").fill(id);
      await page.getByRole("button", { name: "Check" }).click();
      await page.getByRole("button", { name: "Yes, trust it" }).click();
    };
    const senderId = await identify(sender, "Mac");
    const receiverId = await identify(receiver, "Phone");
    await trust(sender, receiverId);
    await trust(receiver, senderId);
    await expect(sender.getByText("connects directly")).toHaveCount(0);

    await enablePreview(sender, /Local network boost/, /Remember trusted devices/);
    /** Starts sending `file` to the phone and returns pictures of the first codes. */
    const send = async (file: string) => {
      await sender.goto("./#/send");
      await sender.locator('input[type="file"]').first().setInputFiles(file);
      await sender.getByRole("checkbox", { name: /Phone/ }).check();
      await sender.getByLabel("Density").selectOption("low");
      await sender.getByRole("button", { name: "Start sending" }).click();
      await expect(sender.getByRole("button", { name: /offering LAN/ })).toBeVisible();
      for (let i = 0; i < 4; i++) await sender.getByRole("button", { name: "Slower" }).click();
      const frames = await capturePlayer(sender, 60);
      expect(frames.length).toBeGreaterThan(30);
      return frames;
    };

    // The first time: the usual way, with the answer code.
    writeY4m(codesVideo, await send(first));
    await sender.getByRole("button", { name: /Two-way/ }).click();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("link-answer")).toBeVisible({ timeout: 30_000 });
    const answer = await captureImage(receiver, "link-answer");
    writeY4m(answerVideo, [answer, answer, answer]);
    await sender.getByRole("button", { name: /Two-way/ }).click();
    await expect(receiver.getByTestId("received-file")).toHaveText(["known-first.bin"], { timeout: 90_000 });
    // The receiver, seeing a sender it trusts, has said who it is; the sender remembers.
    await expect(sender.getByTestId("link-up")).toContainText("Phone");
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
    await sender.getByRole("button", { name: "Done" }).first().click();
    await sender.goto("./#/devices");
    await expect(sender.getByText("connects directly")).toBeVisible();

    // The second time: the sender's camera sees a blank wall throughout.
    writeY4m(answerVideo, [blank()]);
    writeY4m(codesVideo, await send(second));
    await receiver.goto("./#/");
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("link-connected")).toBeVisible({ timeout: 30_000 });
    await expect(sender.getByTestId("link-up")).toBeVisible();
    await expect(receiver.getByTestId("received-file")).toHaveText(["known-second.bin"], { timeout: 90_000 });
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
  } finally {
    await receiverBrowser.close();
    await senderBrowser.close();
  }
});
