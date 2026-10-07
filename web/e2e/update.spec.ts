import { expect, test } from "@playwright/test";
import { FAKE_TEXT, textVideo } from "./fixtures";
import { fakeCamera } from "./video";

// A page that was opened before a new version went live asks for files of
// its own build, which the server no longer has. It must get itself out of
// that, and a hiccup of the network must not look like it.

test("a page left behind by a new version reloads into it", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({ args: fakeCamera(textVideo) });
  try {
    // (Without a service worker, so that the test sees every request.)
    const context = await browser.newContext({ baseURL, permissions: ["camera"], serviceWorkers: "block" });
    const page = await context.newPage();
    await page.goto("./#/devices");
    await page.evaluate(() => ((window as unknown as { openedBefore?: boolean }).openedBefore = true));

    // The new version is live: this page's engine file is gone…
    let live = false;
    await page.route(/qrsend_wasm.*\.wasm/, (route) => (live ? route.continue() : route.fulfill({ status: 404, body: "gone" })));
    // …and the server's page names another build.
    await page.route(/index\.html\?fresh=/, async (route) => {
      const response = await route.fetch();
      live = true;
      await route.fulfill({ response, body: (await response.text()).replace(/index-[^"]+\.js/, "index-NEWBUILD.js") });
    });

    await page.getByRole("link", { name: "Receive", exact: true }).first().click();
    // The page notices, reloads, and receiving works.
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
    expect(await page.evaluate(() => (window as unknown as { openedBefore?: boolean }).openedBefore)).toBeUndefined();
  } finally {
    await browser.close();
  }
});

test("a network hiccup while loading the engine is retried without reloading", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({ args: fakeCamera(textVideo) });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], serviceWorkers: "block" });
    const page = await context.newPage();
    await page.goto("./#/devices");
    await page.evaluate(() => ((window as unknown as { openedBefore?: boolean }).openedBefore = true));
    let failures = 2;
    await page.route(/qrsend_wasm.*\.wasm/, (route) => (failures-- > 0 ? route.abort("connectionreset") : route.continue()));

    await page.getByRole("link", { name: "Receive", exact: true }).first().click();
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
    expect(await page.evaluate(() => (window as unknown as { openedBefore?: boolean }).openedBefore)).toBe(true);
  } finally {
    await browser.close();
  }
});

test("when the engine cannot be loaded at all, the page says what to do", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({ args: fakeCamera(textVideo) });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], serviceWorkers: "block" });
    const page = await context.newPage();
    await page.goto("./#/");
    await page.route(/qrsend_wasm.*\.wasm/, (route) => route.fulfill({ status: 404, body: "gone" }));
    await page.getByRole("link", { name: "Receive", exact: true }).first().click();
    await expect(page.getByText(/could not be loaded.*Reload the page to try again/)).toBeVisible({ timeout: 30_000 });
  } finally {
    await browser.close();
  }
});
