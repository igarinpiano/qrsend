import { expect, test } from "@playwright/test";
import crypto from "node:crypto";
import { broadcast, cameraFrom } from "./bridge";
import { enablePreview } from "./video";

// The sender and the receiver see each other live (see bridge.ts), so the
// sender can react to what the receiver reports.

test("automatic speed: the sender shows more codes once the feedback says they are read", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await cameraFrom(sender, "to-sender");
    await enablePreview(sender, /Two-way transfer/, /Automatic speed/);
    await sender.goto("./#/send");
    // More than one code at a time would carry in the time the test takes.
    const data = crypto.randomBytes(Number(process.env.QRSEND_AUTO_KB ?? 300) * 1000);
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "auto.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByLabel("QR code stream")).toBeVisible();
    await expect(sender.getByText(/1×1/)).toBeVisible();

    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect(receiver.getByTestId("feedback")).toBeVisible({ timeout: 30_000 });
    await broadcast(receiver, "to-sender", '[data-testid="feedback"]', 150, 2);

    // The receiver reads everything shown, so there is room for more: the
    // sender shows several codes at once, and shows them faster.
    await expect(sender.getByTestId("receiver-report")).toBeVisible({ timeout: 30_000 });
    await expect(sender.getByTestId("fps")).toContainText("auto");
    const shown = async () => {
      const text = await sender.locator(".bar .info").first().innerText();
      const [, cols, rows] = /screen (\d+)×(\d+)/.exec(text) ?? [];
      const perPicture = Number(cols) * Number(rows);
      return { perPicture, perSecond: perPicture * parseFloat(await sender.getByTestId("fps").innerText()) };
    };
    // The receiver also reports how its camera sees the codes (how many
    // pictures it reads, how large the dots come out), which takes the sender
    // most of the way in one step instead of many small ones. (Step by step
    // it took eight seconds to get from one code per picture to three.)
    await expect.poll(async () => (await shown()).perPicture, { timeout: 5_000 }).toBeGreaterThanOrEqual(3);
    await expect.poll(async () => (await shown()).perSecond, { timeout: 60_000 }).toBeGreaterThan(20);
    if (process.env.QRSEND_TRACE) {
      for (let i = 0; i < 25; i++) {
        console.log(i * 2, (await sender.locator(".bar .info").first().innerText()).replace(/\s+/g, " "), "|", await sender.getByTestId("fps").innerText());
        await new Promise((r) => setTimeout(r, 2000));
      }
    }
    // And the transfer arrives through all the changes, after which the
    // sender stops by itself.
    await expect(receiver.getByTestId("received-file")).toHaveText(["auto.bin"], { timeout: 60_000 });
    await expect(sender.getByText("The other device has everything.")).toBeVisible({ timeout: 30_000 });
  } finally {
    await browser.close();
  }
});

// Found on real devices: a phone's camera made out only one of the three
// colors of color codes. The data got through (any code will do), but the
// file list, the request for feedback and the offer to connect never did:
// each of them was always drawn in the same one of the other two colors.
for (const [color, name] of [
  [0, "red"],
  [1, "green"],
  [2, "blue"],
] as const) {
  test(`color codes: a camera that makes out only ${name} still learns everything`, async ({ playwright, baseURL }) => {
    const browser = await playwright.chromium.launch({
      args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
    });
    try {
      const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
      const sender = await context.newPage();
      await cameraFrom(sender, "to-sender");
      // An offer to connect makes notices and offers frequent, as it was when this was found.
      await enablePreview(sender, /Two-way transfer/, /Color codes/, /Local network boost/);
      await sender.goto("./#/send");
      await sender
        .locator('input[type="file"]')
        .first()
        .setInputFiles({ name: "one-color.bin", mimeType: "application/octet-stream", buffer: crypto.randomBytes(2_000_000) });
      await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
      await sender.getByLabel("Density").selectOption("low");
      await sender.getByLabel("Codes on screen").selectOption("1");
      await sender.getByRole("button", { name: "Start sending" }).click();
      await expect(sender.getByText("×3 colors")).toBeVisible();

      const receiver = await context.newPage();
      await cameraFrom(receiver, "to-receiver");
      await enablePreview(receiver, /Show measurements/);
      await receiver.goto("./#/receive");
      await broadcast(sender, "to-receiver", "canvas", 50, 1, 24, color);
      // The file list arrives…
      await expect(receiver.getByText("one-color.bin")).toBeVisible({ timeout: 30_000 });
      // …and so does the offer to connect, which the receiver answers.
      await expect(receiver.getByTestId("link-answer")).toBeVisible({ timeout: 30_000 });
      // The measurements say when each step happened and what was read.
      await expect(receiver.getByTestId("rx-steps")).toHaveText(
        /file list after [\d.]+ s · offer to connect read after [\d.]+ s · answer shown\s+after [\d.]+ s · connected not yet\. Of \d+ codes read, \d+ were notices and\s+\d+ offers/,
      );
      console.log(`${name} only: ${(await receiver.getByTestId("rx-steps").innerText()).replace(/\s+/g, " ")}`);
    } finally {
      await browser.close();
    }
  });
}

// A sender cannot see when a receiver starts reading. Found on real devices:
// with the camera ready from the first code everything was there at once,
// but a receiver that turned up a little later waited a long time for the
// file list and for the offer to connect.
test("a receiver that turns up late still gets the file list and the offer soon", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await cameraFrom(sender, "to-sender");
    await enablePreview(sender, /Local network boost/);
    await sender.goto("./#/send");
    // Many files make a long file list: it takes dozens of codes, as that of a very large file does.
    const files = Array.from({ length: 240 }, (_, i) => ({
      name: `photo-${String(i).padStart(4, "0")}-${crypto.randomBytes(6).toString("hex")}.jpg`,
      mimeType: "image/jpeg",
      buffer: crypto.randomBytes(300),
    }));
    await sender.locator('input[type="file"]').first().setInputFiles(files);
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByRole("button", { name: /offering LAN/ })).toBeVisible();
    // The receiver is not there yet.
    await sender.waitForTimeout(20_000);

    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await enablePreview(receiver, /Show measurements/);
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect(receiver.getByTestId("link-answer")).toBeVisible({ timeout: 15_000 });
    // While it waits for the file list, it says how far that is.
    if (!process.env.QRSEND_BASELINE) {
      await expect(receiver.getByTestId("summary")).toHaveText(/Waiting for the file list… \d+ of \d+ codes|240 files/);
    }
    await expect(receiver.getByText(/240 files/)).toBeVisible({ timeout: 60_000 });
    console.log(`late receiver: ${(await receiver.getByTestId("rx-steps").innerText()).replace(/\s+/g, " ")}`);
  } finally {
    await browser.close();
  }
});

// A code the receiver shows to the sender's camera has to be easy to hold
// into view on a phone: large, at the end of the screen where the other
// camera most likely looks, movable to the other end, and full screen on a
// tap. The sender shows what its camera sees large enough to aim by.
test("codes shown to the other device: large, placeable, full screen", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1280, height: 760 } });
    const sender = await context.newPage();
    await cameraFrom(sender, "to-sender");
    await enablePreview(sender, /Local network boost/);
    await sender.goto("./#/send");
    await sender
      .locator('input[type="file"]')
      .first()
      .setInputFiles({ name: "show.bin", mimeType: "application/octet-stream", buffer: crypto.randomBytes(900_000) });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByRole("button", { name: /offering LAN/ })).toBeVisible();
    // While nothing of the receiver has been seen, the sender's camera picture is large, beside the codes.
    const eye = sender.locator("video.eye");
    await expect(eye).toHaveClass(/aiming/);
    await expect(eye).toHaveClass(/beside/);
    expect((await eye.boundingBox())!.width).toBeGreaterThan(200);

    const receiver = await context.newPage();
    await receiver.setViewportSize({ width: 390, height: 760 });
    await cameraFrom(receiver, "to-receiver");
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 60);
    const code = receiver.getByTestId("link-answer");
    const camera = receiver.locator(".camera");
    await expect(code).toBeVisible({ timeout: 30_000 });
    const top = async (of: typeof code) => (await of.boundingBox())!.y;
    // On an upright phone: at the top, nearly as wide as the screen, the camera picture small below it.
    expect(await top(code)).toBeLessThan(await top(camera));
    expect((await code.boundingBox())!.width).toBeGreaterThan(300);
    expect((await camera.boundingBox())!.height).toBeLessThan(200);
    // Moved to the other end, and remembered there.
    await receiver.getByRole("button", { name: "Move to the other end" }).click();
    await expect.poll(async () => (await top(code)) > (await top(camera))).toBe(true);
    expect(await receiver.evaluate(() => localStorage.getItem("qrsend.codePlace"))).toBe("far");
    await receiver.getByRole("button", { name: "Move to the other end" }).click();
    await expect.poll(async () => (await top(code)) < (await top(camera))).toBe(true);
    // Full screen on a tap of the code itself, and back.
    await code.click();
    await expect(receiver.getByRole("button", { name: "Close", exact: true })).toBeVisible();
    expect((await code.boundingBox())!.width).toBeGreaterThan(360);
    await code.click();
    await expect(receiver.getByRole("button", { name: "Full screen", exact: true })).toBeVisible();
    // Lying on its side: code and camera picture side by side.
    await receiver.setViewportSize({ width: 760, height: 390 });
    await expect
      .poll(async () => Math.abs((await top(code)) - (await top(camera))))
      .toBeLessThan(80);
    expect((await code.boundingBox())!.x).toBeLessThan((await camera.boundingBox())!.x);
  } finally {
    await browser.close();
  }
});

// Color codes are three times the codes only for a camera that keeps the
// colors apart. Real cameras fall short in different ways: one reads a
// single color (a real phone did), one loses a color, one takes two for the
// same, one sees no color at all. The receiver reads each color on its own
// and says in its feedback what it finds; the sender keeps to the colors
// that camera tells apart, or goes back to black and white.
for (const [what, camera, note, colors] of [
  ["reads green alone", [-1, 1, -1], /colors off/, 1],
  ["sees no colors (everything as its green)", [1, 1, 1], /colors off/, 1],
  ["does not read red", [-1, 1, 2], /colors: green and blue only/, 2],
  ["does not read blue", [0, 1, -1], /colors: red and green only/, 2],
  ["takes red for green", [1, 1, 2], /colors: green and blue only/, 2],
  ["takes blue for red", [0, 1, 0], /colors: red and green only/, 2],
] as const) {
  test(`color codes: a camera that ${what} is shown what it can tell apart`, async ({ playwright, baseURL }) => {
    test.setTimeout(120_000);
    const browser = await playwright.chromium.launch({
      args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
    });
    try {
      const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
      const sender = await context.newPage();
      await cameraFrom(sender, "to-sender");
      await enablePreview(sender, /Two-way transfer/, /Color codes/);
      await sender.goto("./#/send");
      // Enough to still be going when the colors change.
      const data = crypto.randomBytes(40_000);
      await sender.locator('input[type="file"]').first().setInputFiles({ name: "colors.bin", mimeType: "application/octet-stream", buffer: data });
      await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
      await sender.getByLabel("Density").selectOption("low");
      await sender.getByLabel("Codes on screen").selectOption("1");
      await sender.getByRole("button", { name: "Start sending" }).click();
      await expect(sender.getByText("×3 colors")).toBeVisible();

      const receiver = await context.newPage();
      await cameraFrom(receiver, "to-receiver");
      await receiver.goto("./#/receive");
      await broadcast(sender, "to-receiver", "canvas", 50, 1, 24, camera);
      await expect(receiver.getByTestId("feedback")).toBeVisible({ timeout: 30_000 });
      await broadcast(receiver, "to-sender", '[data-testid="feedback"]', 150, 2);

      await expect(sender.getByTestId("colors-reduced")).toHaveText(note, { timeout: 60_000 });
      await expect(sender.getByText("×3 colors")).toHaveCount(0);
      if (colors === 2) await expect(sender.getByText("×2 colors")).toBeVisible();
      // And it stays at that: what is left, the camera does tell apart.
      await sender.waitForTimeout(6_000);
      await expect(sender.getByTestId("colors-reduced")).toHaveText(note);
      if (colors === 2) await expect(sender.getByText("×2 colors")).toBeVisible();
      await expect(receiver.getByTestId("received-file")).toHaveText(["colors.bin"], { timeout: 90_000 });
    } finally {
      await browser.close();
    }
  });
}

// A camera that tells all three apart keeps all three.
test("color codes: a camera that reads every color keeps them", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await cameraFrom(sender, "to-sender");
    await enablePreview(sender, /Two-way transfer/, /Color codes/);
    await sender.goto("./#/send");
    const data = crypto.randomBytes(60_000);
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "colors.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect(receiver.getByTestId("feedback")).toBeVisible({ timeout: 30_000 });
    await broadcast(receiver, "to-sender", '[data-testid="feedback"]', 150, 2);
    await expect(sender.getByTestId("receiver-report")).toBeVisible({ timeout: 30_000 });
    await sender.waitForTimeout(8_000);
    await expect(sender.getByText("×3 colors")).toBeVisible();
    await expect(sender.getByTestId("colors-reduced")).toHaveCount(0);
  } finally {
    await browser.close();
  }
});

// The hard case: a camera to which color codes are no codes at all (three on
// top of each other, seen without color, are a blur). It reads nothing, so
// it never learns that feedback is wanted, and the sender hears nothing. A
// sender that expects feedback and gets none shows plain codes until it
// does; the colors then get their turn, and when the receiver stops reading
// under them, they are off for good.
test("color codes: a camera that reads nothing in them gets plain codes", async ({ playwright, baseURL }) => {
  test.setTimeout(150_000);
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await cameraFrom(sender, "to-sender");
    await enablePreview(sender, /Two-way transfer/, /Color codes/);
    await sender.goto("./#/send");
    const data = crypto.randomBytes(60_000);
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "blur.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByText("×3 colors")).toBeVisible();

    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50, 1, 24, [-2, -2, -2]);
    await broadcast(receiver, "to-sender", '[data-testid="feedback"]', 150, 2);

    // (The receiver can only come to show its feedback through the plain
    // codes: it reads nothing else. They are shown for a moment only, until
    // the sender hears it.)
    await expect(receiver.getByTestId("feedback")).toBeVisible({ timeout: 30_000 });
    await expect(sender.getByTestId("colors-reduced")).toHaveText(/colors off: the receiver reads nothing while they are shown/, { timeout: 60_000 });
    await expect(receiver.getByTestId("received-file")).toHaveText(["blur.bin"], { timeout: 90_000 });
  } finally {
    await browser.close();
  }
});

// Without feedback the sender cannot know. The receiver can: the sender says
// in its notices that it shows colors, and the receiver sees that its camera
// does not tell them apart. It says so to the person.
test("color codes: without feedback, the receiver says that its camera does not tell them apart", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 700 } });
    const sender = await context.newPage();
    await enablePreview(sender, /Color codes/);
    await sender.goto("./#/send");
    const data = crypto.randomBytes(200_000);
    await sender.locator('input[type="file"]').first().setInputFiles({ name: "one-way.bin", mimeType: "application/octet-stream", buffer: data });
    await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await sender.getByLabel("Density").selectOption("low");
    await sender.getByLabel("Codes on screen").selectOption("1");
    await sender.getByRole("button", { name: "Start sending" }).click();
    await expect(sender.getByText("×3 colors")).toBeVisible();

    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50, 1, 24, [1, 1, 1]);
    await expect(receiver.getByTestId("colors-advice")).toBeVisible({ timeout: 60_000 });
    // The sender, hearing nothing and expecting nothing, goes on as chosen.
    await expect(sender.getByText("×3 colors")).toBeVisible();
  } finally {
    await browser.close();
  }
});
