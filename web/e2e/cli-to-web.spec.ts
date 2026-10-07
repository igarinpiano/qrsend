import { expect, test, type Browser, type Page } from "@playwright/test";
import fs from "node:fs";
import { execFileSync } from "node:child_process";
import {
  FAKE_TEXT,
  SECRET_TEXT,
  encryptedVideo,
  filesVideo,
  hasCli,
  hasFfmpeg,
  noise,
  readIds,
  segmentsVideo,
  textVideo,
  webmVideo,
} from "./fixtures";

test.skip(!hasCli, "qrsend CLI binary not built");

type Fixtures = { playwright: typeof import("@playwright/test"); baseURL: string | undefined };

/** A page whose camera shows the given CLI-rendered video. */
async function withCamera(
  { playwright, baseURL }: Fixtures,
  video: string | undefined,
  run: (page: Page, browser: Browser) => Promise<void>,
) {
  const args = ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"];
  if (video) args.push(`--use-file-for-fake-video-capture=${video}`);
  const browser = await playwright.chromium.launch({ args });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera"], acceptDownloads: true });
    await run(await context.newPage(), browser);
  } finally {
    await browser.close();
  }
}

/** The record is saved asynchronously, so retry until the inbox shows it. */
async function expectInInbox(page: Page) {
  await expect(async () => {
    await page.goto("./#/");
    await page.goto("./#/inbox");
    await expect(page.getByText("Complete")).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 20_000 });
}

test("CLI → web: receives text through the camera", async ({ playwright, baseURL }) => {
  await withCamera({ playwright, baseURL }, textVideo, async (page) => {
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 60_000 });
    await expectInInbox(page);
    // Reopening from the inbox reads it back from storage.
    await page.getByRole("button", { name: "Open" }).click();
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT);
  });
});

test("CLI → web: receives a folder through the camera", async ({ playwright, baseURL }) => {
  await withCamera({ playwright, baseURL }, filesVideo, async (page) => {
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-file")).toHaveText(["notes/a.txt", "notes/b.md"], { timeout: 60_000 });
    await expectInInbox(page);
  });
});

test("CLI → web: many segments are stored, verified and saved byte for byte", async ({ playwright, baseURL }) => {
  await withCamera({ playwright, baseURL }, segmentsVideo, async (page) => {
    await page.goto("./#/receive");
    // While it runs: how much is left, not how much was read.
    await expect(page.getByTestId("remaining")).toHaveText(/\d+% · .+ of .+ · \d+ codes to go/, { timeout: 60_000 });
    await expect(page.getByTestId("received-file")).toHaveText(["multi/big.bin", "multi/sub/note.txt"], { timeout: 90_000 });

    // One file, straight from the output file on disk.
    const [single] = await Promise.all([
      page.waitForEvent("download"),
      page.getByRole("listitem").filter({ hasText: "multi/big.bin" }).getByRole("button", { name: "Save" }).click(),
    ]);
    expect(single.suggestedFilename()).toBe("big.bin");
    expect(fs.readFileSync(await single.path()).equals(noise(40_000))).toBe(true);

    // Everything as a ZIP built without loading the files into memory.
    const [archive] = await Promise.all([
      page.waitForEvent("download"),
      page.getByRole("button", { name: "Download ZIP" }).click(),
    ]);
    const zipPath = await archive.path();
    const listing = execFileSync("unzip", ["-Z1", zipPath], { encoding: "utf8" }).trim().split("\n").sort();
    expect(listing).toEqual(["multi/", "multi/big.bin", "multi/sub/", "multi/sub/note.txt"]);
    execFileSync("unzip", ["-tq", zipPath]);
    const big = execFileSync("unzip", ["-p", zipPath, "multi/big.bin"], { maxBuffer: 1 << 20 });
    expect(big.equals(noise(40_000))).toBe(true);
  });
});

/** Stores an identity the way version 0.1.0 did (a secret string) and the trusted CLI device. */
async function adoptIdentity(page: Page) {
  const ids = readIds();
  await page.goto("./#/");
  // The app creates its database when the home page asks for the inbox.
  await expect(page.getByRole("link", { name: "Receive", exact: true }).first()).toBeVisible();
  await page.evaluate(
    async ({ secret, cliId }) => {
      const open = () =>
        new Promise<IDBDatabase>((resolve, reject) => {
          const req = indexedDB.open("qrsend");
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => reject(req.error);
        });
      let db = await open();
      for (let i = 0; i < 50 && !db.objectStoreNames.contains("kv"); i++) {
        db.close();
        await new Promise((r) => setTimeout(r, 100));
        db = await open();
      }
      const tx = db.transaction("kv", "readwrite");
      tx.objectStore("kv").put(secret, "identity");
      tx.objectStore("kv").put(
        [{ id: cliId, name: "cli", fingerprint: "set-by-test", added: Date.now() }],
        "devices",
      );
      await new Promise<void>((resolve, reject) => {
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(tx.error);
      });
      db.close();
    },
    { secret: ids.webSecret, cliId: ids.cli },
  );
  return ids;
}

test("CLI → web: decrypts with non-extractable keys migrated from an older identity", async ({ playwright, baseURL }) => {
  await withCamera({ playwright, baseURL }, encryptedVideo, async (page) => {
    const ids = await adoptIdentity(page);

    // The identity keeps its ID after moving into WebCrypto…
    await page.goto("./#/devices");
    await page.getByText("Show ID as text").click();
    await expect(page.getByTestId("my-id")).toHaveText(ids.web);
    // …and its private keys are no longer readable by scripts.
    await expect(page.getByText("cannot keep these keys")).toHaveCount(0);
    const stored = await page.evaluate(
      () =>
        new Promise<{ kind?: string; extractable?: boolean; type: string }>((resolve) => {
          const req = indexedDB.open("qrsend");
          req.onsuccess = () => {
            const get = req.result.transaction("kv").objectStore("kv").get("identity");
            get.onsuccess = () =>
              resolve({ kind: get.result?.kind, extractable: get.result?.x25519Private?.extractable, type: typeof get.result });
          };
        }),
    );
    expect(stored).toEqual({ kind: "webcrypto", extractable: false, type: "object" });

    await page.goto("./#/receive");
    await expect(page.getByTestId("received-text")).toHaveText(SECRET_TEXT, { timeout: 60_000 });
  });
});

test("CLI → web: another browser cannot read an encrypted transfer", async ({ playwright, baseURL }) => {
  await withCamera({ playwright, baseURL }, encryptedVideo, async (page) => {
    await page.goto("./#/devices");
    await page.getByRole("button", { name: "Create device ID" }).click();
    await expect(page.getByRole("button", { name: "Copy ID" })).toBeVisible();
    await page.goto("./#/receive");
    await expect(page.getByRole("alert")).toContainText("encrypted for another device", { timeout: 60_000 });
  });
});

test("web: receives from a video file", async ({ playwright, baseURL }) => {
  test.skip(!hasFfmpeg, "ffmpeg not installed");
  await withCamera({ playwright, baseURL }, undefined, async (page) => {
    await page.goto("./#/receive");
    await page.getByTestId("video-file").setInputFiles(webmVideo);
    await expect(page.getByTestId("received-text")).toHaveText(FAKE_TEXT, { timeout: 90_000 });
  });
});
