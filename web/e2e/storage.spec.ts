// The Storage page: what the browser keeps is listed with the room it takes,
// and goes when it is deleted — picked one by one, or all at once.
import { expect, test, type Page } from "@playwright/test";
import path from "node:path";
import { WORK, cli, filesVideo, hasCli, textVideo } from "./fixtures";
import { fakeCamera } from "./video";

/** Files nothing refers to, as a lost record or a crashed tab would leave them. */
async function leaveBehind(page: Page, area: "recv" | "send", name: string, size: number) {
  await page.evaluate(
    async ({ area, name, size }) => {
      const root = await navigator.storage.getDirectory();
      const dir = await (await root.getDirectoryHandle(area, { create: true })).getDirectoryHandle(name, { create: true });
      const out = await (await dir.getFileHandle("body", { create: true })).createWritable();
      await out.write(new Uint8Array(size));
      await out.close();
    },
    { area, name, size },
  );
}

const rows = (page: Page) => page.getByRole("listitem");
const total = (page: Page) => page.getByTestId("storage-total");

test("left-over files are listed with their size and deleted", async ({ page }) => {
  await page.goto("./#/");
  await leaveBehind(page, "recv", "deadbeef", 300_000);
  await leaveBehind(page, "recv", "0badcafe", 5_000);
  await page.goto("./#/storage");

  // The largest first; the log is always there.
  await expect(rows(page)).toHaveCount(3);
  await expect(rows(page).nth(0)).toContainText("Left over");
  await expect(rows(page).nth(0)).toContainText("293.0 KiB");
  await expect(rows(page).nth(1)).toContainText("4.88 KiB");
  await expect(rows(page).nth(2)).toContainText("Diagnostic log");
  await expect(total(page)).toContainText("KiB");

  // Nothing picked, nothing to delete. One picked: that one goes, after a question.
  const remove = page.getByRole("button", { name: /^Delete selected/ });
  await expect(remove).toBeDisabled();
  await rows(page).nth(0).getByRole("checkbox").check();
  await expect(remove).toHaveText("Delete selected (1 · 293.0 KiB)");
  page.once("dialog", (d) => {
    expect(d.message()).toContain("Left over");
    d.dismiss();
  });
  await remove.click();
  await expect(rows(page)).toHaveCount(3);
  page.once("dialog", (d) => d.accept());
  await remove.click();
  await expect(page.getByRole("status")).toHaveText("Removed 293.0 KiB.");
  await expect(rows(page)).toHaveCount(2);
  expect(await page.evaluate(async () => (await (await navigator.storage.getDirectory()).getDirectoryHandle("recv")).getDirectoryHandle("deadbeef").then(() => true, () => false))).toBe(false);

  // All at once, the log with it.
  await page.getByRole("button", { name: "Select all" }).click();
  await expect(page.getByRole("button", { name: "Select none" })).toBeVisible();
  await expect(remove).toHaveText(/^Delete selected \(2 · /);
  page.once("dialog", (d) => d.accept());
  await remove.click();
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).nth(0)).toContainText("Diagnostic log");

  // The page fits a narrow screen.
  await page.setViewportSize({ width: 320, height: 568 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
});

test("the log can be cleared", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("./#/");
  // Something for the log: a few pages.
  for (const path of ["inbox", "devices", "preview", "storage"]) await page.goto(`./#/${path}`);
  const log = rows(page).filter({ hasText: "Diagnostic log" });
  await expect(log).toContainText(/[5-9] lines/);
  await log.getByRole("checkbox").check();
  page.once("dialog", (d) => d.accept());
  await page.getByRole("button", { name: /^Delete selected/ }).click();
  await expect(page.getByRole("status")).toContainText("Removed");
  await page.getByTestId("copy-log").click();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  // The heading stays; of what happened, only what came after the clearing.
  expect(copied).toContain("QRSend ");
  expect(copied).not.toContain("path=/devices");
  expect(copied).toContain("removed on the storage page");
});

test("received transfers are listed and leave the inbox when deleted", async ({ playwright, baseURL }) => {
  test.skip(!hasCli, "qrsend CLI binary not built");
  const received: [string, string][] = [
    [textVideo, "received-text"],
    [filesVideo, "received-file"],
  ];
  // One browser profile for both transfers: what the first left must still be there.
  const dir = test.info().outputPath("profile");
  for (const [video, shown] of received) {
    const context = await playwright.chromium.launchPersistentContext(dir, {
      // (The browser the configuration names, if it names one.)
      channel: test.info().project.use.channel,
      args: fakeCamera(video),
      baseURL,
      permissions: ["camera"],
    });
    try {
      const page = context.pages()[0] ?? (await context.newPage());
      await page.goto("./#/receive");
      await expect(page.getByTestId(shown).first()).toBeVisible({ timeout: 60_000 });
      // (The record is saved a moment after the result shows.)
      await expect(async () => {
        await page.goto("./#/");
        await page.goto("./#/inbox");
        await expect(page.getByText("Complete")).toHaveCount(received.findIndex(([v]) => v === video) + 1, { timeout: 2_000 });
      }).toPass({ timeout: 20_000 });
      if (video !== filesVideo) continue;

      await page.getByRole("link", { name: "See what takes the room" }).click();
      await expect(rows(page)).toHaveCount(3);
      const files = rows(page).filter({ hasText: "notes" });
      await expect(files).toContainText("Received, complete");
      await expect(files).toContainText(/\d+ B|KiB/);
      // Deleting one leaves the other, here and in the inbox.
      await files.getByRole("checkbox").check();
      page.once("dialog", (d) => d.accept());
      await page.getByRole("button", { name: /^Delete selected/ }).click();
      await expect(rows(page)).toHaveCount(2);
      await page.goto("./#/inbox");
      await expect(page.getByText("Complete")).toHaveCount(1);
      await expect(page.getByText("notes")).toHaveCount(0);
      // And all of it.
      await page.goto("./#/storage");
      await page.getByRole("button", { name: "Select all" }).click();
      page.once("dialog", (d) => d.accept());
      await page.getByRole("button", { name: /^Delete selected/ }).click();
      await expect(rows(page)).toHaveCount(1);
      await page.goto("./#/inbox");
      await expect(page.getByText("Nothing received yet.")).toBeVisible();
    } finally {
      await context.close();
    }
  }
});

/** Every file under `recv/` in the browser's file storage, as "session/name". */
const receivedFiles = (page: Page) =>
  page.evaluate(async () => {
    type Dir = FileSystemDirectoryHandle & { entries(): AsyncIterable<[string, FileSystemHandle]> };
    const out: string[] = [];
    const root = await navigator.storage.getDirectory();
    const recv = await root.getDirectoryHandle("recv").catch(() => undefined);
    if (!recv) return out;
    for await (const [session, dir] of (recv as Dir).entries()) {
      for await (const [name] of (dir as Dir).entries()) out.push(`${session}/${name}`);
    }
    return out;
  });

test("a transfer that stopped halfway is deleted with all that was kept of it", async ({ playwright, baseURL }) => {
  test.skip(!hasCli, "qrsend CLI binary not built");
  // Less than half of a pass, over and over: it can never complete.
  const video = path.join(WORK, "halfway.y4m");
  cli(["send", "--plain", "in/multi", "--seg-shift", "12", "--density", "low", "--scale", "4", "--grid", "2", "--export-video", video, "--frames", "18"]);
  const browser = await playwright.chromium.launch({ channel: test.info().project.use.channel, args: fakeCamera(video) });
  try {
    const page = await (await browser.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await page.goto("./#/receive");
    await expect(page.getByTestId("remaining")).toBeVisible({ timeout: 60_000 });
    // Until the camera has shown all there is, and it is written down.
    await expect(async () => {
      const files = await receivedFiles(page);
      expect(files.some((f) => f.endsWith("/body"))).toBe(true);
      expect(files.some((f) => /\/part-\d+$/.test(f))).toBe(true);
    }).toPass({ timeout: 30_000 });
    await page.waitForTimeout(1_500);

    // Leaving the page keeps it, unfinished, for later.
    await page.goto("./#/inbox");
    await expect(page.getByRole("link", { name: "Continue" })).toBeVisible();
    await page.getByRole("link", { name: "See what takes the room" }).click();
    const unfinished = rows(page).filter({ hasText: "Received in part" });
    await expect(unfinished).toHaveCount(1);
    await expect(unfinished).toContainText(/\(\d+ of \d+\)/);
    await expect(unfinished).toContainText("KiB");
    expect((await receivedFiles(page)).length).toBeGreaterThan(1);

    // Deleted: the parts received, the codes of the parts begun, the record.
    await unfinished.getByRole("checkbox").check();
    page.once("dialog", (d) => d.accept());
    await page.getByRole("button", { name: /^Delete selected/ }).click();
    await expect(rows(page)).toHaveCount(1);
    expect(await receivedFiles(page)).toEqual([]);
    await page.goto("./#/inbox");
    await expect(page.getByText("Nothing received yet.")).toBeVisible();
  } finally {
    await browser.close();
  }
});
