// The player, the scanner and the decoder in a browser.
import { chromium, expect, test } from "@playwright/test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

async function loop(page, query) {
  await page.goto(`loop.html?${query}`);
  await expect(page.locator("#status")).toHaveText("done", { timeout: 90_000 });
  const result = await page.evaluate(() => window.result);
  console.log(query, JSON.stringify(result));
  return result;
}

for (const worker of ["1", "0"]) {
  test(`what the player shows, the scanner reads (worker ${worker === "1" ? "on" : "off"})`, async ({ page }) => {
    const r = await loop(page, `size=30000&worker=${worker}`);
    expect(r).toMatchObject({ kind: "files", paths: ["folder/data.bin", "hello.txt"], same: true, hello: "hello" });
    // Headless Chromium on Linux has no BarcodeDetector for QR codes.
    expect(["ZXing", "BarcodeDetector"]).toContain(r.engine);
    expect(r.updates).toBeGreaterThan(1);
    // The codes were found on a worker, or on the page.
    expect(page.workers().length).toBe(worker === "1" ? 1 : 0);
  });
}

test("several codes in one picture", async ({ page }) => {
  const r = await loop(page, "size=60000&grid=4");
  expect(r).toMatchObject({ same: true });
});

const cli = process.env.QRSEND_BIN ?? path.join(root, "target/debug", process.platform === "win32" ? "qrsend.exe" : "qrsend");

test("the camera reads what the CLI shows", async () => {
  test.skip(!existsSync(cli), `no CLI at ${cli} (cargo build -p qrsend)`);
  const dir = mkdtempSync(path.join(tmpdir(), "qrsend-web-e2e-"));
  writeFileSync(path.join(dir, "from-cli.txt"), "hello from the CLI, through a camera");
  const video = path.join(dir, "codes.y4m");
  const env = {
    ...process.env,
    QRSEND_DATA_DIR: path.join(dir, "data"),
    QRSEND_CACHE_DIR: path.join(dir, "cache"),
    QRSEND_LOG: "off",
  };
  const sent = spawnSync(cli, ["send", path.join(dir, "from-cli.txt"), "--plain", "--export-y4m", video, "--passes", "3"], { env, encoding: "utf8" });
  expect(sent.status, sent.stderr).toBe(0);
  // A browser whose camera plays that video.
  const browser = await chromium.launch({
    executablePath: process.env.QRSEND_CHROMIUM || undefined,
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", `--use-file-for-fake-video-capture=${video}`],
  });
  try {
    const page = await browser.newPage({ baseURL: "http://localhost:4174/" });
    await page.goto("receive.html");
    await expect(page.locator("#trademark")).toHaveText(/DENSO WAVE INCORPORATED/);
    await page.locator("#start").click();
    await expect(page.locator("#status")).toHaveText("Received.", { timeout: 60_000 });
    await expect(page.locator("#received a")).toHaveText(/^from-cli\.txt \(36 bytes\)$/);
  } finally {
    await browser.close();
  }
});

/** The number of the picture on the send page's canvas, and how many were made. */
async function shown(page) {
  const status = await page.locator("#status").textContent();
  const m = status.match(/Picture (\d+)(?: of (\d+))?/);
  return { picture: Number(m[1]), pictures: Number(m[2] ?? m[1]), paused: status.includes("paused") };
}

const canvasImage = (page) => page.locator("#codes").evaluate((c) => c.toDataURL());

test("the player pauses, steps forward and back, and changes speed", async ({ page }) => {
  await page.goto("index.html");
  // A text that takes many codes.
  let text = "";
  for (let i = 0; i < 20000; i++) text += String.fromCharCode(97 + ((i * 7919) % 26) ^ (i % 3));
  await page.locator("#text").fill(text);
  await page.locator("#send").click();
  await expect(page.locator("#status")).toHaveText(/Picture \d+/);
  // A few pictures in, so that there are some to step back to.
  await expect.poll(async () => (await shown(page)).picture).toBeGreaterThanOrEqual(3);

  // Paused: the picture stays.
  await page.locator("#toggle").click();
  await expect(page.locator("#toggle")).toHaveText("Play");
  const paused = await shown(page);
  expect(paused.paused).toBe(true);
  const before = await canvasImage(page);
  await page.waitForTimeout(500);
  expect(await shown(page)).toEqual(paused);
  expect(await canvasImage(page)).toBe(before);

  // Forward: a new picture. Back: the one before, as it was.
  await page.locator("#forward").click();
  expect(await shown(page)).toEqual({ picture: paused.picture + 1, pictures: paused.pictures + 1, paused: true });
  const next = await canvasImage(page);
  expect(next).not.toBe(before);
  await page.locator("#back").click();
  expect(await shown(page)).toEqual({ picture: paused.picture, pictures: paused.pictures + 1, paused: true });
  expect(await canvasImage(page)).toBe(before);
  await page.locator("#back").click();
  expect((await shown(page)).picture).toBe(paused.picture - 1);
  // Forward again goes through the pictures shown before, then to new ones.
  await page.locator("#forward").click();
  await page.locator("#forward").click();
  expect(await canvasImage(page)).toBe(next);
  await page.locator("#forward").click();
  expect(await shown(page)).toEqual({ picture: paused.picture + 2, pictures: paused.pictures + 2, paused: true });
  // The keyboard: space plays.
  await page.locator("#codes").click();
  await page.keyboard.press("Space");
  await expect(page.locator("#toggle")).toHaveText("Pause");

  // The speed, in milliseconds per picture.
  const rate = async (ms, seconds) => {
    await page.locator("#interval").fill(String(ms));
    await page.locator("#interval").dispatchEvent("change");
    const a = (await shown(page)).pictures;
    await page.waitForTimeout(seconds * 1000);
    return ((await shown(page)).pictures - a) / seconds;
  };
  const slow = await rate(500, 3);
  expect(slow).toBeGreaterThanOrEqual(1.3);
  expect(slow).toBeLessThanOrEqual(2.7);
  const fast = await rate(50, 2);
  expect(fast).toBeGreaterThanOrEqual(12);
  expect(fast).toBeLessThanOrEqual(22);
});

test("the send page shows codes, with the trademark notice", async ({ page }) => {
  await page.goto("index.html");
  await expect(page.locator("#trademark")).toHaveText(/QR Code is a registered trademark of DENSO WAVE INCORPORATED/);
  await page.locator("#text").fill("hello");
  await page.locator("#send").click();
  await expect(page.locator("#status")).toHaveText(/codes made/);
});
