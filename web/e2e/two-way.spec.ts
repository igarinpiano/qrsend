import { expect, test, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { WORK } from "./fixtures";

// The whole loop between two browsers: the sender's codes reach the receiver
// through its (fake) camera, and the receiver's feedback code reaches the
// sender through the sender's (fake) camera.

type Gray = { width: number; height: number; luma: string };

/** Writes grayscale frames as the uncompressed video Chrome's fake camera plays. */
function writeY4m(file: string, frames: Gray[], fps = 10) {
  const w = frames[0].width & ~1;
  const h = frames[0].height & ~1;
  const chroma = Buffer.alloc((w * h) / 2, 128);
  const parts: Buffer[] = [Buffer.from(`YUV4MPEG2 W${w} H${h} F${fps}:1 Ip A1:1 C420jpeg\n`)];
  for (const f of frames) {
    const src = Buffer.from(f.luma, "base64");
    const y = Buffer.alloc(w * h);
    for (let row = 0; row < h; row++) src.copy(y, row * w, row * f.width, row * f.width + w);
    parts.push(Buffer.from("FRAME\n"), y, chroma);
  }
  fs.writeFileSync(file, Buffer.concat(parts));
}

/** Grayscale pixels of the player's canvas, once per displayed frame. */
async function capturePlayer(page: Page, count: number): Promise<Gray[]> {
  await expect(page.getByLabel("QR code stream")).toBeVisible();
  return page.evaluate(async (wanted) => {
    const canvas = document.querySelector("canvas")!;
    const ctx = canvas.getContext("2d")!;
    const out: { width: number; height: number; luma: string }[] = [];
    let last = canvas.dataset.frames;
    const until = Date.now() + 30_000;
    while (out.length < wanted && Date.now() < until) {
      await new Promise((r) => setTimeout(r, 20));
      if (canvas.dataset.frames === last) continue;
      last = canvas.dataset.frames;
      const { width, height } = canvas;
      const rgba = ctx.getImageData(0, 0, width, height).data;
      let bin = "";
      for (let i = 0; i < rgba.length; i += 4) bin += String.fromCharCode(rgba[i]);
      out.push({ width, height, luma: btoa(bin) });
    }
    return out;
  }, count);
}

/** Grayscale pixels of an image on the page, enlarged for the camera. */
async function captureImage(page: Page, testId: string): Promise<Gray> {
  return page.getByTestId(testId).evaluate(async (img: HTMLImageElement) => {
    await img.decode();
    const scale = 2;
    const canvas = document.createElement("canvas");
    canvas.width = img.naturalWidth * scale;
    canvas.height = img.naturalHeight * scale;
    const ctx = canvas.getContext("2d")!;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    const rgba = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let bin = "";
    for (let i = 0; i < rgba.length; i += 4) bin += String.fromCharCode(rgba[i]);
    return { width: canvas.width, height: canvas.height, luma: btoa(bin) };
  });
}

test("without the preview feature nothing changes: no camera on the sender, no feedback on the receiver", async ({ page }) => {
  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill("plain");
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByRole("button", { name: "Start sending" }).click();
  await expect(page.getByLabel("QR code stream")).toBeVisible();
  await expect(page.getByRole("button", { name: /Two-way/ })).toHaveCount(0);
});

test("two-way: the sender stops once the receiver's feedback says everything arrived", async ({ playwright, baseURL }) => {
  const message = "Two-way transfer ↔ with feedback";
  fs.mkdirSync(WORK, { recursive: true });
  const codesVideo = path.join(WORK, "two-way-codes.y4m");
  const feedbackVideo = path.join(WORK, "two-way-feedback.y4m");
  const blank: Gray = { width: 64, height: 64, luma: Buffer.alloc(64 * 64, 255).toString("base64") };
  writeY4m(feedbackVideo, [blank]);

  const camera = (video: string) => [
    "--use-fake-ui-for-media-stream",
    "--use-fake-device-for-media-stream",
    `--use-file-for-fake-video-capture=${video}`,
  ];
  const senderBrowser = await playwright.chromium.launch({ args: camera(feedbackVideo) });
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    // Two-way transfer is a preview feature, turned on by the sender.
    const sender = await (await senderBrowser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await sender.goto("./#/preview");
    await sender.getByRole("checkbox", { name: /Two-way transfer/ }).check();
    await sender.goto("./#/send");
    await expect(sender.getByText("Two-way transfer is on")).toBeVisible();
    await sender.getByRole("tab", { name: "Text" }).click();
    await sender.getByPlaceholder("Paste or type anything…").fill(message);
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByRole("button", { name: "Start sending" }).click();
    // Its stream now carries the data plus a notice asking for feedback.
    const frames = await capturePlayer(sender, 24);
    expect(frames.length).toBeGreaterThan(16);
    writeY4m(codesVideo, frames);
    // Nothing answers yet (the camera sees a blank wall): release the camera.
    await sender.getByRole("button", { name: /Two-way/ }).click();

    // The receiver needs no setting: asked by the notice, it answers with a feedback code.
    receiverBrowser = await playwright.chromium.launch({ args: camera(codesVideo) });
    const context = await receiverBrowser.newContext({ baseURL, permissions: ["camera"] });
    const receiver = await context.newPage();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("received-text")).toHaveText(message, { timeout: 60_000 });
    await expect(receiver.getByTestId("feedback")).toHaveAttribute("data-code", /^QSF1-/);
    await expect(receiver.getByText("so it knows everything arrived")).toBeVisible();
    const code = await captureImage(receiver, "feedback");
    writeY4m(feedbackVideo, [code, code, code]);

    // The sender's camera sees it: nothing is missing, so it stops.
    await expect(sender.getByText("The other device has everything.")).toBeHidden();
    await sender.getByRole("button", { name: /Two-way/ }).click();
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
  } finally {
    await receiverBrowser?.close();
    await senderBrowser.close();
  }
});
