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
