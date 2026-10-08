import { expect, test, type Page } from "@playwright/test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { WORK } from "./fixtures";
import { broadcast, cameraFrom } from "./bridge";

// A transfer that stopped halfway. The receiver kept what it had (Inbox →
// Continue); what does the sender do? It sends the same data again: for an
// unencrypted transfer that is the same session, bit for bit, so the receiver
// carries on where it stopped. Nothing is asked of the sender but the data,
// and no channel back to it is needed.

async function send(sender: Page, file: string) {
  await sender.goto("./#/");
  await sender.goto("./#/send");
  await sender.locator('input[type="file"]').first().setInputFiles(file);
  await sender.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await sender.getByLabel("Density").selectOption("low");
  await sender.getByLabel("Codes on screen").selectOption("3");
  await sender.getByRole("button", { name: "Start sending" }).click();
  await expect(sender.getByLabel("QR code stream")).toBeVisible();
}

const percent = async (receiver: Page) => parseInt((await receiver.getByTestId("remaining").innerText().catch(() => "0")) || "0", 10) || 0;

test("an unfinished transfer continues when the sender sends the same data again", async ({ playwright, baseURL }) => {
  test.setTimeout(240_000);
  fs.mkdirSync(WORK, { recursive: true });
  const first = path.join(WORK, "continue-a.bin");
  const other = path.join(WORK, "continue-b.bin");
  const data = crypto.randomBytes(450_000);
  const otherData = crypto.randomBytes(40_000);
  fs.writeFileSync(first, data);
  fs.writeFileSync(other, otherData);
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 760 }, acceptDownloads: true });
    const sender = await context.newPage();
    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");

    // Part of it arrives, then the sender stops.
    await send(sender, first);
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect.poll(() => percent(receiver), { timeout: 60_000 }).toBeGreaterThan(15);
    await sender.getByRole("button", { name: "Done" }).click();
    const session = (await receiver.locator(".badge").first().innerText()).trim();
    expect(session).toMatch(/^[0-9a-f]{8}$/);
    const had = await percent(receiver);
    expect(had).toBeLessThan(100);

    // The inbox says how to carry on, for both kinds of sender, with the code to read off the screen.
    await receiver.goto("./#/inbox");
    await receiver.getByRole("button", { name: "How to continue" }).click();
    await expect(receiver.getByTestId("resume-code")).toContainText("send the very same data again");
    await expect(receiver.getByTestId("resume-text")).toHaveText(/^qrsend send --resume QSR1-[A-Z2-7]+$/);
    await expect(receiver.getByAltText("The resume command as a QR code")).toBeVisible();
    // The receiver comes back to the transfer; what it has is still there.
    await receiver.getByRole("link", { name: "Continue" }).first().click();
    await expect(receiver).toHaveURL(new RegExp(`session=${session}`));
    // (Also of the megabyte it was in the middle of: the codes read of it were kept.)
    await expect.poll(() => percent(receiver), { timeout: 15_000 }).toBeGreaterThanOrEqual(had - 1);

    // The sender shows something else: the receiver says so instead of silently waiting.
    await send(sender, other);
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect(receiver.getByTestId("other-transfer")).toBeVisible({ timeout: 30_000 });
    await expect(receiver.getByTestId("other-transfer")).toContainText("The sender is showing another transfer");
    expect(await percent(receiver)).toBeGreaterThanOrEqual(had - 1);
    await sender.getByRole("button", { name: "Done" }).click();

    // The sender sends the first data again: the same transfer, carried on from where it was.
    await send(sender, first);
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect(receiver.getByTestId("other-transfer")).toBeHidden({ timeout: 30_000 });
    await expect.poll(() => percent(receiver), { timeout: 30_000 }).toBeGreaterThan(had);
    await expect(receiver.getByTestId("received-file")).toHaveText(["continue-a.bin"], { timeout: 120_000 });
    const [download] = await Promise.all([
      receiver.waitForEvent("download"),
      receiver.getByRole("listitem").filter({ hasText: "continue-a.bin" }).getByRole("button", { name: "Save" }).click(),
    ]);
    expect(fs.readFileSync(await download.path()).equals(data)).toBe(true);

    // Sent yet again to a receiver that has it all: recognized at the first code, nothing to receive.
    await receiver.goto("./#/");
    await receiver.goto("./#/receive");
    await expect(receiver.getByTestId("already-received")).toBeVisible({ timeout: 30_000 });
    await expect(receiver.getByTestId("received-file")).toHaveText(["continue-a.bin"]);
  } finally {
    await browser.close();
  }
});

test("a receiver waiting for one transfer can take the one being shown instead", async ({ playwright, baseURL }) => {
  fs.mkdirSync(WORK, { recursive: true });
  const first = path.join(WORK, "switch-a.bin");
  const other = path.join(WORK, "switch-b.bin");
  fs.writeFileSync(first, crypto.randomBytes(700_000));
  const otherData = crypto.randomBytes(40_000);
  fs.writeFileSync(other, otherData);
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], viewport: { width: 1000, height: 760 }, acceptDownloads: true });
    const sender = await context.newPage();
    const receiver = await context.newPage();
    await cameraFrom(receiver, "to-receiver");
    await send(sender, first);
    await receiver.goto("./#/receive");
    await broadcast(sender, "to-receiver", "canvas", 50);
    await expect.poll(() => percent(receiver), { timeout: 60_000 }).toBeGreaterThan(5);
    await sender.getByRole("button", { name: "Done" }).click();
    await send(sender, other);
    await broadcast(sender, "to-receiver", "canvas", 50);
    await receiver.getByRole("button", { name: "Receive the other transfer instead" }).click();
    await expect(receiver.getByTestId("received-file")).toHaveText(["switch-b.bin"], { timeout: 60_000 });
    // What was received of the first one is still in the inbox.
    await receiver.goto("./#/inbox");
    await expect(receiver.getByRole("link", { name: "Continue" })).toHaveCount(1);
  } finally {
    await browser.close();
  }
});
