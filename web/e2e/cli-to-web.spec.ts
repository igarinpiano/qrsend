import { expect, test } from "@playwright/test";
import { FAKE_TEXT, filesVideo, hasCli, textVideo } from "./fixtures";

test.skip(!hasCli, "qrsend CLI binary not built");

for (const [name, video] of [
  ["text", textVideo],
  ["folder", filesVideo],
] as const) {
  test(`CLI → web: receives ${name} through the camera`, async ({ playwright, baseURL }) => {
    // The CLI-rendered video becomes the browser's camera.
    const browser = await playwright.chromium.launch({
      args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", `--use-file-for-fake-video-capture=${video}`],
    });
    try {
      const context = await browser.newContext({ baseURL, permissions: ["camera"] });
      const page = await context.newPage();
      await page.goto("./#/receive");
      if (name === "text") {
        await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
      } else {
        await expect(page.getByTestId("received-file")).toHaveText(["notes/a.txt", "notes/b.md"], { timeout: 60_000 });
      }
      // It is kept in the inbox.
      // The record is saved asynchronously, so retry until it shows up.
      await expect(async () => {
        await page.goto("./#/");
        await page.goto("./#/inbox");
        await expect(page.getByText("Complete")).toBeVisible({ timeout: 2_000 });
      }).toPass({ timeout: 20_000 });
    } finally {
      await browser.close();
    }
  });
}
