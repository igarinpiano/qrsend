// A live link between two pages of one browser context: what one page shows
// is what the other page's camera sees, as it happens. (The fake camera of
// the browser can only play a file recorded beforehand, which is no use for
// testing how one side reacts to the other.)
import type { Page } from "@playwright/test";

/**
 * Gives the page a camera that shows whatever is broadcast on `channel`.
 * Call before the page loads.
 */
export async function cameraFrom(page: Page, channel: string) {
  await page.addInitScript((name) => {
    const canvas = document.createElement("canvas");
    canvas.width = 640;
    canvas.height = 480;
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    new BroadcastChannel(name).onmessage = (e: MessageEvent<ImageBitmap>) => {
      const picture = e.data;
      if (canvas.width !== picture.width || canvas.height !== picture.height) {
        canvas.width = picture.width;
        canvas.height = picture.height;
      }
      ctx.drawImage(picture, 0, 0);
      picture.close();
    };
    // A camera delivers pictures even when nothing moves.
    let flip = false;
    setInterval(() => {
      flip = !flip;
      ctx.fillStyle = flip ? "#fff" : "#fefefe";
      ctx.fillRect(0, 0, 1, 1);
    }, 40);
    navigator.mediaDevices.getUserMedia = async () => canvas.captureStream(30);
  }, channel);
}

/**
 * Broadcasts the element matching `selector` (a canvas or an image) on
 * `channel` every `everyMs`, enlarged by `scale` on a white ground `margin`
 * pixels wide all around.
 */
export async function broadcast(page: Page, channel: string, selector: string, everyMs: number, scale = 1, margin = 24) {
  await page.evaluate(
    ([name, sel, every, zoom, margin]) => {
      const out = new BroadcastChannel(name);
      const stage = document.createElement("canvas");
      const ctx = stage.getContext("2d")!;
      setInterval(async () => {
        const el = document.querySelector(sel) as HTMLCanvasElement | HTMLImageElement | null;
        if (!el) return;
        const [w, h] = el instanceof HTMLImageElement ? [el.naturalWidth, el.naturalHeight] : [el.width, el.height];
        if (!w || !h) return;
        stage.width = w * zoom + 2 * margin;
        stage.height = h * zoom + 2 * margin;
        ctx.fillStyle = "#fff";
        ctx.fillRect(0, 0, stage.width, stage.height);
        ctx.imageSmoothingEnabled = false;
        ctx.drawImage(el, margin, margin, w * zoom, h * zoom);
        out.postMessage(await createImageBitmap(stage));
      }, every);
    },
    [channel, selector, everyMs, scale, margin] as const,
  );
}
