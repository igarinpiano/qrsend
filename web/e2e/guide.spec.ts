import { expect, test } from "@playwright/test";
import crypto from "node:crypto";
import { Guide, modulesFor, type Box, type Look } from "../src/lib/guide";
import { broadcast, cameraFrom } from "./bridge";
import { enablePreview } from "./video";

// Advice on holding the camera: first the reasoning on its own, then once
// with a real decoder looking at a real picture.

const W = 1280;
const H = 720;
/** A code of 600 characters (77 dots a side at least) drawn `side` pixels wide at `x`, `y`. */
const code = (x: number, y: number, side: number): Box => ({ x0: x, y0: y, x1: x + side, y1: y + side, chars: 600 });
const look = (boxes: Box[], sharp = 1000): Look => ({ width: W, height: H, boxes, sharp });
/** The advice after seeing the same picture for a while. */
const settle = (guide: Guide, picture: Look, times = 30) => {
  let advice;
  for (let i = 0; i < times; i++) advice = guide.notice(picture);
  return advice;
};

test("the size of a code follows from the length of its text", () => {
  expect(modulesFor(20)).toBe(21);
  expect(modulesFor(600)).toBe(69);
  expect(modulesFor(4000)).toBe(173);
});

test("advice: closer, back, fewer, or nothing when all is well", () => {
  // Large and well inside the picture: nothing to say.
  expect(settle(new Guide(), look([code(400, 100, 500)]))).toBeUndefined();
  // Small, with room all around: come closer.
  expect(settle(new Guide(), look([code(560, 280, 160)]))).toBe("closer");
  // Reaching the edge: some of the grid may be outside.
  expect(settle(new Guide(), look([code(4, 100, 500)]))).toBe("back");
  // Many small codes filling the picture: closer would not help.
  const grid = [0, 1, 2, 3, 4].flatMap((c) => [0, 1, 2].map((r) => code(60 + c * 235, 20 + r * 230, 210)));
  expect(settle(new Guide(), look(grid))).toBe("fewer");
});

test("advice: a blurred picture is told from one that merely shows no codes", () => {
  const guide = new Guide();
  settle(guide, look([code(400, 100, 500)], 1000));
  // No codes, as sharp as before: the camera points elsewhere. Not blur.
  expect(settle(guide, look([], 900))).toBeUndefined();
  // No codes and far less detail than when codes were read: blur.
  expect(settle(guide, look([], 150))).toBe("blurred");
  // Reading again: the advice goes away.
  expect(settle(guide, look([code(400, 100, 500)], 950))).toBeUndefined();
});

test("advice does not flicker", () => {
  const guide = new Guide();
  const small = look([code(560, 280, 160)]);
  const fine = look([code(400, 100, 500)]);
  // One odd picture among good ones says nothing…
  settle(guide, fine);
  expect(guide.notice(small)).toBeUndefined();
  expect(settle(guide, fine, 3)).toBeUndefined();
  // …and once advice shows, one good picture does not take it away.
  expect(settle(guide, small)).toBe("closer");
  expect(guide.notice(fine)).toBe("closer");
  expect(guide.notice(small)).toBe("closer");
});

test("camera guidance: a real decoder, a code far away", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await sender.goto("./#/send");
    await sender
      .locator('input[type="file"]')
      .first()
      .setInputFiles({ name: "far.bin", mimeType: "application/octet-stream", buffer: crypto.randomBytes(200_000) });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByLabel("QR code stream")).toBeVisible();

    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await enablePreview(receiver, /Camera guidance/);
    await receiver.goto("./#/receive");
    // The sender's screen, small in the middle of a large picture.
    await broadcast(sender, "to-receiver", "canvas", 80, 0.4, 500);
    await expect(receiver.getByTestId("camera-advice")).toHaveText(/Move closer/, { timeout: 30_000 });
    // It is read all the same: advice, not an error.
    await expect(receiver.getByTestId("remaining")).toContainText("codes", { timeout: 30_000 });
  } finally {
    await browser.close();
  }
});

test("camera guidance is silent unless its preview feature is on", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const page = await (await browser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await page.goto("./#/receive");
    await expect(page.getByText(/Point the camera/)).toBeVisible();
    await expect(page.getByTestId("camera-advice")).toHaveCount(0);
  } finally {
    await browser.close();
  }
});
