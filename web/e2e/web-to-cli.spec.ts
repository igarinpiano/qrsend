import { expect, test } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { WORK, cli, hasCli } from "./fixtures";

test.skip(!hasCli, "qrsend CLI binary not built");

test("web → CLI: frames rendered by the browser decode in the CLI", async ({ page }) => {
  const message = "Sent from the browser 🌐";
  await page.goto("./#/send");
  await page.getByRole("tab", { name: "Text" }).click();
  await page.getByPlaceholder("Paste or type anything…").fill(message);
  await page.getByRole("checkbox", { name: /Anyone who sees the codes/ }).check();
  await page.getByLabel("Density").selectOption("low");
  await page.getByRole("button", { name: "Start sending" }).click();
  await expect(page.getByLabel("QR code stream")).toBeVisible();

  const frames: string[] = await page.evaluate(async () => {
    const canvas = document.querySelector("canvas")!;
    const out: string[] = [];
    let last = canvas.dataset.frames;
    const until = Date.now() + 30_000;
    while (out.length < 12 && Date.now() < until) {
      await new Promise((r) => setTimeout(r, 20));
      if (canvas.dataset.frames !== last) {
        last = canvas.dataset.frames;
        out.push(canvas.toDataURL("image/png"));
      }
    }
    return out;
  });
  expect(frames.length).toBeGreaterThan(4);

  const dir = path.join(WORK, "web-frames");
  fs.mkdirSync(dir, { recursive: true });
  frames.forEach((url, i) =>
    fs.writeFileSync(path.join(dir, `f${String(i).padStart(3, "0")}.png`), Buffer.from(url.split(",")[1], "base64")),
  );
  expect(cli(["recv", "--images", dir])).toContain(message);
});
