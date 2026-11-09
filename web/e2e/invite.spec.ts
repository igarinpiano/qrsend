// For a receiver without QRSend: an ordinary QR code that opens the Receive
// page with a phone's camera app, on the Send page and over the stream.
import { expect, test } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { prepareZXingModule, readBarcodes } from "zxing-wasm/reader";
import { PUBLISHED, receiveLink } from "../src/lib/invite";

/** What an ordinary QR reader finds in a picture given as a data: URL. */
async function read(dataUrl: string): Promise<string[]> {
  prepareZXingModule({
    overrides: {
      wasmBinary: fs.readFileSync(path.join(import.meta.dirname, "../node_modules/zxing-wasm/dist/reader/zxing_reader.wasm"))
        .buffer as ArrayBuffer,
    },
  });
  const png = Buffer.from(dataUrl.replace(/^data:image\/png;base64,/, ""), "base64");
  const found = await readBarcodes(new Blob([png], { type: "image/png" }), { formats: ["QRCode"] });
  return found.map((b) => b.text);
}

const at = (url: string) => {
  const u = new URL(url);
  return { protocol: u.protocol, hostname: u.hostname, origin: u.origin, pathname: u.pathname };
};

test("the link is to the copy of the app others can reach", () => {
  // The published site, and a copy someone hosts: that very copy.
  expect(receiveLink(at("https://igarinpiano.github.io/qrsend/"))).toBe(`${PUBLISHED}#/receive`);
  expect(receiveLink(at("https://igarinpiano.github.io/qrsend/index.html"))).toBe(`${PUBLISHED}#/receive`);
  expect(receiveLink(at("https://qr.example.org/tools/qrsend/"))).toBe("https://qr.example.org/tools/qrsend/#/receive");
  // Where nobody else can reach it, the published site.
  for (const here of [
    "http://localhost:5173/",
    "https://localhost:4173/qrsend/",
    "file:///Users/me/qrsend-web-0.1.15/index.html",
    "http://192.168.1.20:5173/",
    "https://192.168.1.20/",
    "https://10.0.0.5/qrsend/",
    "https://mac.local/qrsend/",
    "https://[fd12:3456::1]/",
    "https://127.0.0.1/",
  ])
    expect(receiveLink(at(here)), here).toBe(`${PUBLISHED}#/receive`);
});

test("the Send page offers the code, and the player shows it over the stream", async ({ page }) => {
  await page.goto("./#/send");
  await page.getByText("The receiver has no QRSend open?").click();
  const invite = page.getByTestId("invite");
  // (The tests run on a local server: nobody else could open that.)
  await expect(invite.getByTestId("invite-link")).toHaveText(`${PUBLISHED}#/receive`);
  const code = invite.getByTestId("invite-code");
  await expect(code).toBeVisible();
  expect(await read((await code.getAttribute("src"))!)).toEqual([`${PUBLISHED}#/receive`]);
  await expect(invite.getByRole("button", { name: "Copy link" })).toBeVisible();

  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill("hello");
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByRole("button", { name: "Start sending" }).click();
  const canvas = page.getByLabel("QR code stream");
  const frames = async () => Number(await canvas.getAttribute("data-frames"));
  await expect.poll(frames).toBeGreaterThan(2);

  // Over the stream, large; the stream waits meanwhile.
  await page.getByTestId("invite-open").click();
  const big = page.locator(".player").getByTestId("invite");
  await expect(big).toBeVisible();
  const shown = (await big.getByTestId("invite-code").boundingBox())!;
  expect(shown.width).toBeGreaterThan(250);
  const held = await frames();
  await page.waitForTimeout(1000);
  expect(await frames()).toBe(held);

  // Back: the stream goes on. (And L does the same as the button.)
  await page.getByTestId("invite-back").click();
  await expect(big).toBeHidden();
  await expect.poll(frames).toBeGreaterThan(held);
  await page.keyboard.press("l");
  await expect(big).toBeVisible();
  await page.keyboard.press("l");
  await expect(big).toBeHidden();
});

test("on a phone, the code over the stream fits the screen", async ({ browser, baseURL }) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 390, height: 844 }, deviceScaleFactor: 3 });
  const page = await context.newPage();
  try {
    await page.goto("./#/send");
    await page.getByRole("tab", { name: "Text" }).click();
    await page.getByPlaceholder("Paste or type anything…").fill("hello");
    await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
    await page.getByRole("button", { name: "Start sending" }).click();
    await page.getByTestId("invite-open").click();
    const big = page.locator(".player").getByTestId("invite");
    const code = (await big.getByTestId("invite-code").boundingBox())!;
    const back = (await page.getByTestId("invite-back").boundingBox())!;
    expect(code.x).toBeGreaterThanOrEqual(0);
    expect(code.x + code.width).toBeLessThanOrEqual(390);
    expect(code.y).toBeGreaterThanOrEqual(0);
    expect(code.width).toBeGreaterThan(200);
    // The way back is on the screen, above the bar.
    const bar = (await page.locator(".player .bar").boundingBox())!;
    expect(back.y + back.height).toBeLessThanOrEqual(bar.y + 1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: path.join("test-results", "invite-phone.png") });
  } finally {
    await context.close();
  }
});
