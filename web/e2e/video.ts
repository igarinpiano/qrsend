// Turns what one page shows into the video another page's fake camera plays.
import { expect, type Page } from "@playwright/test";
import fs from "node:fs";

/** A picture as base64 RGBA. */
export type Picture = { width: number; height: number; rgba: string };

/** Writes pictures as the uncompressed 4:2:0 video Chrome's fake camera plays. */
export function writeY4m(file: string, pictures: Picture[], fps = 10) {
  const w = pictures[0].width & ~1;
  const h = pictures[0].height & ~1;
  const parts: Buffer[] = [Buffer.from(`YUV4MPEG2 W${w} H${h} F${fps}:1 Ip A1:1 C420jpeg\n`)];
  for (const p of pictures) {
    const src = Buffer.from(p.rgba, "base64");
    const y = Buffer.alloc(w * h);
    const u = Buffer.alloc((w * h) / 4);
    const v = Buffer.alloc((w * h) / 4);
    for (let row = 0; row < h; row += 2) {
      for (let col = 0; col < w; col += 2) {
        let su = 0;
        let sv = 0;
        for (const [dy, dx] of [[0, 0], [0, 1], [1, 0], [1, 1]]) {
          const i = ((row + dy) * p.width + col + dx) * 4;
          const [r, g, b] = [src[i], src[i + 1], src[i + 2]];
          // BT.601, full range (what "C420jpeg" means).
          y[(row + dy) * w + col + dx] = Math.round(0.299 * r + 0.587 * g + 0.114 * b);
          su += -0.168736 * r - 0.331264 * g + 0.5 * b + 128;
          sv += 0.5 * r - 0.418688 * g - 0.081312 * b + 128;
        }
        const c = (row / 2) * (w / 2) + col / 2;
        u[c] = Math.max(0, Math.min(255, Math.round(su / 4)));
        v[c] = Math.max(0, Math.min(255, Math.round(sv / 4)));
      }
    }
    parts.push(Buffer.from("FRAME\n"), y, u, v);
  }
  fs.writeFileSync(file, Buffer.concat(parts));
}

export function blank(size = 64): Picture {
  return { width: size, height: size, rgba: Buffer.alloc(size * size * 4, 255).toString("base64") };
}

/** Browser arguments for a camera that plays `video`. */
export const fakeCamera = (video: string) => [
  "--use-fake-ui-for-media-stream",
  "--use-fake-device-for-media-stream",
  `--use-file-for-fake-video-capture=${video}`,
];

/** The player's canvas, once per displayed frame. */
export async function capturePlayer(page: Page, count: number): Promise<Picture[]> {
  await expect(page.getByLabel("QR code stream")).toBeVisible();
  return page.evaluate(async (wanted) => {
    const canvas = document.querySelector("canvas")!;
    const ctx = canvas.getContext("2d")!;
    const out: { width: number; height: number; rgba: string }[] = [];
    let last = canvas.dataset.frames;
    const until = Date.now() + 30_000;
    while (out.length < wanted && Date.now() < until) {
      await new Promise((r) => setTimeout(r, 20));
      if (canvas.dataset.frames === last) continue;
      last = canvas.dataset.frames;
      const { width, height } = canvas;
      const data = ctx.getImageData(0, 0, width, height).data;
      let bin = "";
      for (let i = 0; i < data.length; i += 0x8000) bin += String.fromCharCode(...data.subarray(i, i + 0x8000));
      out.push({ width, height, rgba: btoa(bin) });
    }
    return out;
  }, count);
}

/** An image on the page, enlarged for the camera. */
export async function captureImage(page: Page, testId: string): Promise<Picture> {
  return page.getByTestId(testId).evaluate(async (img: HTMLImageElement) => {
    await img.decode();
    const scale = 2;
    const canvas = document.createElement("canvas");
    canvas.width = img.naturalWidth * scale;
    canvas.height = img.naturalHeight * scale;
    const ctx = canvas.getContext("2d")!;
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let bin = "";
    for (let i = 0; i < data.length; i += 0x8000) bin += String.fromCharCode(...data.subarray(i, i + 0x8000));
    return { width: canvas.width, height: canvas.height, rgba: btoa(bin) };
  });
}

/** Turns preview features on, through the page a user would use. */
export async function enablePreview(page: Page, ...titles: RegExp[]) {
  await page.goto("./#/preview");
  for (const title of titles) await page.getByRole("checkbox", { name: title }).check();
}
