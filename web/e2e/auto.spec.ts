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
      await receiver.goto("./#/receive");
      await broadcast(sender, "to-receiver", "canvas", 50, 1, 24, color);
      // The file list arrives…
      await expect(receiver.getByText("one-color.bin")).toBeVisible({ timeout: 30_000 });
      // …and so does the offer to connect, which the receiver answers.
      await expect(receiver.getByTestId("link-answer")).toBeVisible({ timeout: 30_000 });
    } finally {
      await browser.close();
    }
  });
}
