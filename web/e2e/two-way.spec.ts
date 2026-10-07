import { expect, test } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { WORK } from "./fixtures";
import { blank, captureImage, capturePlayer, enablePreview, fakeCamera as camera, writeY4m } from "./video";

// The whole loop between two browsers: the sender's codes reach the receiver
// through its (fake) camera, and the receiver's feedback code reaches the
// sender through the sender's (fake) camera.

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
  writeY4m(feedbackVideo, [blank()]);
  const senderBrowser = await playwright.chromium.launch({ args: camera(feedbackVideo) });
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    // Two-way transfer is a preview feature, turned on by the sender.
    const sender = await (await senderBrowser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await enablePreview(sender, /Two-way transfer/);
    await sender.goto("./#/send");
    await expect(sender.getByText("Two-way transfer is on")).toBeVisible();
    await sender.getByRole("tab", { name: "Text" }).click();
    await sender.getByPlaceholder("Paste or type anything…").fill(message);
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByRole("button", { name: "Start sending" }).click();
    // Its stream now carries the data plus a notice asking for feedback.
    // Slow enough that the capture misses no frame: the notice is only in some of them.
    for (let i = 0; i < 4; i++) await sender.getByRole("button", { name: "Slower" }).click();
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

/** A WAV file of silence (16-bit mono), for a microphone that hears nothing yet. */
function silence(file: string, seconds = 1, rate = 48000) {
  const header = Buffer.alloc(44);
  const bytes = seconds * rate * 2;
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + bytes, 4);
  header.write("WAVEfmt ", 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(rate, 24);
  header.writeUInt32LE(rate * 2, 28);
  header.writeUInt16LE(2, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(bytes, 40);
  fs.writeFileSync(file, Buffer.concat([header, Buffer.alloc(bytes)]));
}

test("feedback by sound: the receiver's chirps reach the sender's microphone", async ({ playwright, baseURL }) => {
  const message = "Heard, not seen 🔊";
  fs.mkdirSync(WORK, { recursive: true });
  const codesVideo = path.join(WORK, "sound-codes.y4m");
  const heard = path.join(WORK, "sound-heard.wav");
  silence(heard);

  // The sender has a microphone and no camera worth mentioning.
  const microphone = ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", `--use-file-for-fake-audio-capture=${heard}`];
  const senderBrowser = await playwright.chromium.launch({ args: microphone });
  let receiverBrowser: Awaited<ReturnType<typeof playwright.chromium.launch>> | undefined;
  try {
    const sender = await (await senderBrowser.newContext({ baseURL, permissions: ["microphone"] })).newPage();
    await enablePreview(sender, /Feedback by sound/);
    await sender.goto("./#/send");
    await sender.getByRole("tab", { name: "Text" }).click();
    await sender.getByPlaceholder("Paste or type anything…").fill(message);
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByRole("button", { name: /Two-way 🎤/ })).toBeVisible();
    for (let i = 0; i < 4; i++) await sender.getByRole("button", { name: "Slower" }).click();
    const frames = await capturePlayer(sender, 24);
    writeY4m(codesVideo, frames);
    // Nothing to hear yet: release the microphone.
    await sender.getByRole("button", { name: /Two-way/ }).click();

    // The receiver is asked before it makes a sound, then answers with the feedback code as chirps.
    receiverBrowser = await playwright.chromium.launch({ args: camera(codesVideo) });
    // (The page's content security policy keeps scripts from reading the sound back; the test may.)
    const receiver = await (await receiverBrowser.newContext({ baseURL, permissions: ["camera"], bypassCSP: true })).newPage();
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("received-text")).toHaveText(message, { timeout: 60_000 });
    await expect(receiver.getByTestId("feedback-sound")).toHaveAttribute("data-code", "");
    await receiver.getByRole("button", { name: "Answer by sound" }).click();
    await expect(receiver.getByTestId("feedback-sound")).toHaveAttribute("data-code", /^QSF1-/);
    const wav = await receiver.getByTestId("feedback-sound").evaluate(async (audio: HTMLAudioElement) => {
      const bytes = new Uint8Array(await (await fetch(audio.src)).arrayBuffer());
      let bin = "";
      for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
      return btoa(bin);
    });
    fs.writeFileSync(heard, Buffer.from(wav, "base64"));

    // The sender's microphone hears it: nothing is missing, so it stops.
    await sender.getByRole("button", { name: /Two-way/ }).click();
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
  } finally {
    await receiverBrowser?.close();
    await senderBrowser.close();
  }
});
