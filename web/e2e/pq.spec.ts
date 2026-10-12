// Device IDs with post-quantum keys (docs/POST-QUANTUM.md, PROTOCOL §9.1):
// some 3 kB of keys, shown as several QR codes one after the other and put
// together again by whoever reads them, in the browser and on the command
// line alike.
import { expect, test, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { WORK, cli, cliBoth, hasCli } from "./fixtures";
import { captureImage, fakeCamera, writeY4m, type Picture } from "./video";

test.skip(!hasCli, "qrsend CLI binary not built");

/** Creates this browser's identity and returns its ID and fingerprint. */
async function identity(page: Page, name: string) {
  await page.goto("./#/devices");
  await page.getByLabel("Device name").fill(name);
  await page.getByRole("button", { name: "Create device ID" }).click();
  await page.getByText("Show ID as text").click();
  const id = (await page.getByTestId("my-id").textContent())!;
  const fingerprint = (await page.locator(".me .fpr").textContent())!.trim();
  return { id, fingerprint };
}

/** A picture as a PNG file, and back (the page does the encoding: it has what it takes). */
async function toPng(page: Page, picture: Picture, file: string) {
  const data = await page.evaluate(({ width, height, rgba }) => {
    const canvas = document.createElement("canvas");
    [canvas.width, canvas.height] = [width, height];
    const bytes = Uint8ClampedArray.from(atob(rgba), (c) => c.charCodeAt(0));
    canvas.getContext("2d")!.putImageData(new ImageData(bytes, width, height), 0, 0);
    return canvas.toDataURL("image/png").split(",")[1];
  }, picture);
  fs.writeFileSync(file, Buffer.from(data, "base64"));
}

async function fromPng(page: Page, file: string): Promise<Picture> {
  const url = `data:image/png;base64,${fs.readFileSync(file).toString("base64")}`;
  return page.evaluate(async (url) => {
    const img = new Image();
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    [canvas.width, canvas.height] = [img.naturalWidth, img.naturalHeight];
    const ctx = canvas.getContext("2d")!;
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let bin = "";
    for (let i = 0; i < data.length; i += 0x8000) bin += String.fromCharCode(...data.subarray(i, i + 0x8000));
    return { width: canvas.width, height: canvas.height, rgba: btoa(bin) };
  }, url);
}

/** Every code of the ID this page shows, stepped through by hand. */
async function idCodes(page: Page): Promise<Picture[]> {
  const code = page.getByTestId("my-id-code");
  const parts = Number(await code.getAttribute("data-parts"));
  const pictures: Picture[] = [];
  for (let n = 0; n < parts; n++) {
    await page.getByRole("button", { name: "Next code" }).click();
    pictures.push(await captureImage(page, "my-id-code"));
  }
  return pictures;
}

test("an ID with post-quantum keys is shown as several codes, and read from them", async ({ playwright, baseURL }) => {
  const dir = path.join(WORK, "pq");
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  const video = path.join(dir, "id.y4m");
  writeY4m(video, [{ width: 64, height: 64, rgba: Buffer.alloc(64 * 64 * 4, 255).toString("base64") }]);
  const shows = await playwright.chromium.launch();
  const reads = await playwright.chromium.launch({ args: fakeCamera(video) });
  try {
    const first = await (await shows.newContext({ baseURL })).newPage();
    const mine = await identity(first, "first browser");
    // All four keys: a long ID, a fingerprint of five groups, several codes
    // that turn by themselves until one is held.
    expect(mine.id).toMatch(/^qrsend-id:2:/);
    expect(mine.id.length).toBeGreaterThan(4000);
    expect(mine.fingerprint).toMatch(/^\d{4}(-\d{4}){4}$/);
    const code = first.getByTestId("my-id-code");
    const parts = Number(await code.getAttribute("data-parts"));
    expect(parts).toBeGreaterThanOrEqual(8);
    const before = await code.getAttribute("data-part");
    await expect(code).not.toHaveAttribute("data-part", before!, { timeout: 3000 });
    await first.getByRole("button", { name: "Hold" }).click();
    const held = await code.getAttribute("data-part");
    await first.waitForTimeout(1600);
    await expect(code).toHaveAttribute("data-part", held!);
    const pictures = await idCodes(first);
    expect(pictures).toHaveLength(parts);

    // Another browser reads them with its camera, out of order and with
    // one of them late: it says how far it is, then shows the fingerprint.
    const late = [...pictures.slice(1).reverse(), ...pictures.slice(1), pictures[0]];
    writeY4m(video, late.flatMap((p) => [p, p, p]), 6);
    const second = await (await reads.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await second.goto("./#/devices");
    await second.getByRole("button", { name: "Scan its QR code" }).click();
    await expect(second.getByTestId("id-scan-progress")).toContainText(`of ${parts} codes`, { timeout: 30_000 });
    await expect(second.getByText("Does the other device show this fingerprint?")).toBeVisible({ timeout: 60_000 });
    await expect(second.locator(".confirm .fpr")).toHaveText(mine.fingerprint);
    await second.getByRole("button", { name: "Yes, trust it" }).click();
    await expect(second.getByText("first browser")).toBeVisible();
    await expect(second.getByText("older keys only")).toHaveCount(0);

    // The command line reads the same codes from pictures…
    const files: string[] = [];
    for (const [n, picture] of pictures.entries()) {
      files.push(path.join(dir, `web-${n}.png`));
      await toPng(first, picture, files[n]);
    }
    const added = cliBoth(["devices", "add", "--yes", "--name", "browser", "--image", ...files.reverse()], "pq-cli");
    expect(added.stderr).toContain(`Fingerprint: ${mine.fingerprint}`);
    expect(added.status).toBe(0);
    // …and the browser reads the command line's.
    cli(["id", "--name", "pq-cli", "--no-qr", "--png", path.join(dir, "cli.png")], "pq-cli");
    const shown = cli(["id", "--no-qr"], "pq-cli");
    const cliFingerprint = /Fingerprint:\s+([\d-]+)/.exec(shown)![1];
    const theirs: Picture[] = [];
    for (const f of fs.readdirSync(dir).filter((f) => /^cli-\d+\.png$/.test(f))) {
      theirs.push(await fromPng(first, path.join(dir, f)));
    }
    expect(theirs.length).toBeGreaterThanOrEqual(8);
    writeY4m(video, theirs.flatMap((p) => [p, p, p]), 6);
    const third = await (await reads.newContext({ baseURL, permissions: ["camera"] })).newPage();
    await third.goto("./#/devices");
    await third.getByRole("button", { name: "Scan its QR code" }).click();
    await expect(third.locator(".confirm .fpr")).toHaveText(cliFingerprint, { timeout: 60_000 });
  } finally {
    await shows.close();
    await reads.close();
  }
});

test("a device trusted with its older keys takes the new ones into the same record", async ({ page }) => {
  cli(["id", "--name", "pq-old", "--no-qr"], "pq-old");
  const id = (flag: string[]) =>
    cli(["id", "--no-qr", ...flag], "pq-old")
      .split("\n")
      .find((l) => l.startsWith("ID:"))!
      .slice(3)
      .trim();
  const [classic, full] = [id(["--classic"]), id([])];
  await page.goto("./#/devices");
  const add = async (text: string) => {
    await page.getByPlaceholder("qrsend-id:…").fill(text);
    await page.getByRole("button", { name: "Check" }).click();
    await page.getByRole("button", { name: "Yes, trust it" }).click();
  };
  await add(classic);
  await expect(page.getByText("older keys only")).toBeVisible();
  await add(full);
  await expect(page.getByTestId("trust-note")).toContainText("now has post-quantum keys");
  await expect(page.getByText("older keys only")).toHaveCount(0);
  await expect(page.locator(".list li")).toHaveCount(1);
  // The older ID once more is refused: it would take the keys away again.
  await page.getByPlaceholder("qrsend-id:…").fill(classic);
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Yes, trust it" }).click();
  await expect(page.getByText("this is its older ID").first()).toBeVisible();
});
