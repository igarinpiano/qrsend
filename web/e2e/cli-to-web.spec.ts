import { expect, test, type Browser, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { blank, writeY4m } from "./video";
import { execFileSync } from "node:child_process";
import {
  FAKE_TEXT,
  WORK,
  cli,
  cliBoth,
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

// A command-line sender that reads feedback (`send --two-way`, or
// `--feedback-from` as here) asks for it in its stream; the browser needs no
// setting to answer: it shows what it still lacks, and at the end that it
// has everything.
test("CLI → web: a sender that asks for feedback is answered", async ({ playwright, baseURL }) => {
  const video = path.join(WORK, "asks.y4m");
  const nobody = path.join(WORK, "nobody.txt");
  fs.writeFileSync(nobody, "");
  const message = "Asked by the command line";
  cli(["send", "--plain", "--text", message, "--density", "low", "--feedback-from", nobody, "--export-video", video, "--frames", "40", "--fps", "8"]);
  await withCamera({ playwright, baseURL }, video, async (page) => {
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-text")).toHaveText(message, { timeout: 40_000 });
    await expect(page.getByTestId("feedback")).toHaveAttribute("data-code", /^QSF1-/);
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
    await expect(page.getByTestId("remaining")).toHaveText(/\d+% · .+ of .+ · \d+ of \d+\s+codes, \d+ to go/, { timeout: 60_000 });
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
async function adoptIdentity(page: Page, knownSender?: string) {
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
      // Wait until the app has created its database: opening it earlier
      // would create an empty one here, which the app then cannot upgrade.
      for (let i = 0; i < 100; i++) {
        if ((await indexedDB.databases()).some((d) => d.name === "qrsend" && (d.version ?? 0) >= 2)) break;
        await new Promise((r) => setTimeout(r, 100));
      }
      const db = await open();
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
    { secret: ids.webSecret, cliId: knownSender ?? ids.cli },
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
    // Both devices know each other with their post-quantum keys, and the
    // page says so: how it was encrypted, and how the signature was checked.
    await expect(page.getByTestId("protection")).toHaveText("post-quantum");
    await expect(page.getByTestId("sender-keys")).toHaveText("post-quantum signature");
    await expect(page.getByTestId("older-keys-warning")).toHaveCount(0);
  });
});

// A sender that knows this browser by its ID from before post-quantum keys
// encrypts the older way; a sender this browser knows by such an ID is
// checked the older way. Both are said, as warnings, where the transfer is
// received.
test("CLI → web: older keys in use are warned of", async ({ playwright, baseURL }) => {
  const ids = readIds();
  const classic = (device: string) =>
    cli(["id", "--classic", "--no-qr"], device)
      .split("\n")
      .find((l) => l.startsWith("ID:"))!
      .slice(3)
      .trim();
  const video = path.join(WORK, "older.y4m");
  cli(["id", "--name", "older-cli", "--no-qr"], "older-cli");
  cli(["devices", "add", classic("webdev"), "--yes", "--name", "web"], "older-cli");
  const sent = cliBoth(
    ["send", "--to", "web", "--text", SECRET_TEXT, "--density", "low", "--scale", "4", "--export-video", video, "--passes", "3"],
    "older-cli",
  );
  expect(sent.stderr).toContain("WARNING");
  expect(sent.stderr).toContain("the older way only");
  await withCamera({ playwright, baseURL }, video, async (page) => {
    // This browser knows that sender by its older ID as well.
    await adoptIdentity(page, classic("older-cli"));
    await page.goto("./#/devices");
    await expect(page.getByTestId("older-devices-warning")).toBeVisible();
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-text")).toHaveText(SECRET_TEXT, { timeout: 60_000 });
    await expect(page.getByTestId("protection")).toHaveText("older encryption");
    await expect(page.getByTestId("sender-keys")).toHaveText("older signature only");
    await expect(page.getByTestId("older-keys-warning")).toContainText("encrypted the older way only");
    await expect(page.getByTestId("older-keys-warning")).toContainText("its signature was checked the older way only");
    expect(ids.web).toMatch(/^qrsend-id:2:/);
  });
});

// Forward secrecy the other way: the browser shows a one-time key, the
// command line encrypts to it as well, and the browser destroys the key once
// the transfer has arrived.
test("CLI → web: forward secrecy with the browser's one-time key", async ({ playwright, baseURL }) => {
  const video = path.join(WORK, "forward.y4m");
  writeY4m(video, [blank()]);
  await withCamera({ playwright, baseURL }, video, async (page) => {
    await adoptIdentity(page);
    await page.goto("./#/receive");
    await page.getByTestId("one-time-show").click();
    // Three or four codes, turning; and the same as a line of text.
    await expect(page.getByTestId("one-time-code")).toHaveAttribute("data-parts", /^[34]$/);
    const key = (await page.getByTestId("one-time-text").textContent())!;
    expect(key).toMatch(/^QSK1-/);
    const pending = () =>
      page.evaluate(
        () =>
          new Promise<number>((resolve) => {
            const req = indexedDB.open("qrsend");
            req.onsuccess = () => {
              const get = req.result.transaction("kv").objectStore("kv").get("one-time");
              get.onsuccess = () => resolve((get.result ?? []).length);
            };
          }),
      );
    expect(await pending()).toBe(1);
    const sent = cliBoth(
      ["send", "--to", "webdev", "--one-time-key", key, "--text", SECRET_TEXT, "--density", "low", "--scale", "4", "--export-video", video, "--passes", "3"],
      "cli",
    );
    expect(sent.stderr).toContain("forward secrecy and post-quantum protection");
    // (The test's camera plays the file it was started with: the page is
    // opened anew for the video that exists now. The key was kept, so the
    // page that showed it need not be the one that receives.)
    const context = page.context();
    await page.close();
    const receiving = await context.newPage();
    await receiving.goto("./#/receive");
    await expect(receiving.getByTestId("received-text")).toHaveText(SECRET_TEXT, { timeout: 60_000 });
    await expect(receiving.getByTestId("protection")).toHaveText("forward secrecy · post-quantum");
    await expect(receiving.getByTestId("older-keys-warning")).toHaveCount(0);
    // Arrived: the key is destroyed.
    await expect
      .poll(() =>
        receiving.evaluate(
          () =>
            new Promise<number>((resolve) => {
              const req = indexedDB.open("qrsend");
              req.onsuccess = () => {
                const get = req.result.transaction("kv").objectStore("kv").get("one-time");
                get.onsuccess = () => resolve((get.result ?? []).length);
              };
            }),
        ),
      )
      .toBe(0);
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

test("the diagnostic log tells how a transfer went, not what was in it", async ({ playwright, baseURL }) => {
  const browser = await playwright.chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", `--use-file-for-fake-video-capture=${filesVideo}`],
  });
  try {
    const context = await browser.newContext({ baseURL, permissions: ["camera", "clipboard-read", "clipboard-write"] });
    const page = await context.newPage();
    await page.goto("./#/receive");
    await expect(page.getByTestId("received-file")).toHaveText(["notes/a.txt", "notes/b.md"], { timeout: 60_000 });
    await page.getByTestId("copy-log").click();
    await expect(page.getByTestId("copy-log")).toContainText("Copied");
    const text = await page.evaluate(() => navigator.clipboard.readText());
    // Which build, which browser, and the steps with their timings.
    expect(text).toMatch(/^QRSend \d+\.\d+\.\d+ \(built /);
    expect(text).toContain("browser: ");
    expect(text).toMatch(/camera +started/);
    expect(text).toMatch(/camera +reading +source=camera picture=\d+×\d+ decoder=\w+/);
    expect(text).toMatch(/rx +transfer found +segments=\d+/);
    expect(text).toMatch(/rx +file list +bytes=\d+/);
    expect(text).toMatch(/rx +unpacked +ms=\d+ kind=files/);
    expect(text).toMatch(/rx +done/);
    // Nothing of what was transferred.
    expect(text).not.toContain("a.txt");
    expect(text).not.toContain("notes");
    expect(text).not.toContain("qrsend-id");
    // A reload does not lose it: the session before is still there.
    await page.reload();
    await page.getByTestId("copy-log").click();
    await expect(page.getByTestId("copy-log")).toContainText("Copied");
    const again = await page.evaluate(() => navigator.clipboard.readText());
    expect(again).toContain("the session before this one");
    expect(again).toMatch(/rx +done/);
  } finally {
    await browser.close();
  }
});
