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

test("the send page shows codes, with the trademark notice", async ({ page }) => {
  await page.goto("index.html");
  await expect(page.locator("#trademark")).toHaveText(/QR Code is a registered trademark of DENSO WAVE INCORPORATED/);
  await page.locator("#text").fill("hello");
  await page.locator("#send").click();
  await expect(page.locator("#status")).toHaveText(/codes shown/);
});
