import { expect, test, webkit } from "@playwright/test";

// WebKit is what Safari and every browser on iPhone and iPad run on. It
// differs from Chrome in places that matter here, so the basics are checked
// in it too.

test("WebKit: a device ID can be created and is still there after a reload", async ({ baseURL }) => {
  const browser = await webkit.launch();
  try {
    const page = await (await browser.newContext({ baseURL })).newPage();
    await page.goto("./#/devices");
    // WebKit on a Mac or an iPhone creates the keys but returns nothing when
    // they are read back from IndexedDB; the page must then say that it keeps
    // them as site data and use that way. (Other builds may manage either.)
    await expect(page.getByText(/kept as site data|cannot be exported/)).toBeVisible();
    await page.getByRole("button", { name: "Create device ID" }).click();
    await expect(page.getByTestId("my-id")).toHaveText(/^qrsend-id:1:age1/);
    const id = await page.getByTestId("my-id").textContent();
    await page.reload();
    await expect(page.getByTestId("my-id")).toHaveText(id!);
  } finally {
    await browser.close();
  }
});

test("WebKit: the engine packs a text and the player shows codes", async ({ baseURL }) => {
  const browser = await webkit.launch();
  try {
    const page = await (await browser.newContext({ baseURL })).newPage();
    await page.goto("./#/send");
    await page.getByRole("tab", { name: "Text" }).click();
    await page.getByPlaceholder("Paste or type anything…").fill("WebKit works");
    await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await page.getByRole("button", { name: "Start sending" }).click();
    // The engine (WebAssembly in a worker, OPFS) packs the text and the player draws codes.
    const canvas = page.getByLabel("QR code stream");
    await expect(canvas).toBeVisible();
    const first = Number(await canvas.getAttribute("data-frames"));
    await expect.poll(async () => Number(await canvas.getAttribute("data-frames"))).toBeGreaterThan(first);
  } finally {
    await browser.close();
  }
});
