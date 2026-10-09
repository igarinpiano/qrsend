// The Feature preview page.
import { expect, test } from "@playwright/test";

// Connecting directly can mean one network, or the internet (on mobile
// data, at the cost of the data plan): the sender chooses, with the menu
// its system gives it.
test("the direct connection is off, within one network, or through the internet as well", async ({ page }) => {
  await page.goto("./#/preview");
  const menu = page.getByRole("combobox", { name: "Local network boost" });
  await expect(menu.locator("option")).toHaveText(["Off", "Wi-Fi", "Wi-Fi & Cellular Data"]);
  await expect(menu).toHaveValue("");

  await menu.selectOption({ label: "Wi-Fi & Cellular Data" });
  expect(await page.evaluate(() => localStorage.getItem("qrsend.preview.lan"))).toBe("any");
  await page.reload();
  await expect(menu).toHaveValue("any");

  await menu.selectOption({ label: "Off" });
  expect(await page.evaluate(() => localStorage.getItem("qrsend.preview.lan"))).toBeNull();

  // Whoever had it on before it became a choice gets the careful one.
  await page.evaluate(() => localStorage.setItem("qrsend.preview.lan", "1"));
  await page.reload();
  await expect(menu).toHaveValue("wifi");
  // The other previews are still switches.
  await expect(page.getByRole("checkbox", { name: /Two-way transfer/ })).not.toBeChecked();
});

// Found on a phone: the menu took the whole width of its card and pushed the
// feature's name out of it. On any screen, everything stays inside its card
// and the page never scrolls sideways.
for (const [width, height] of [
  [280, 600],
  [320, 568],
  [390, 844],
  [430, 932],
  [768, 1024],
  [1440, 900],
] as const) {
  test(`the preview page fits a screen ${width} wide`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto("./#/preview");
    const menu = page.getByRole("combobox", { name: "Local network boost" });
    await menu.selectOption({ label: "Wi-Fi & Cellular Data" });
    const card = menu.locator("xpath=ancestor::div[contains(@class, 'card')]");
    const inside = async (what: ReturnType<typeof page.locator>) => {
      const [outer, inner] = [await card.boundingBox(), await what.boundingBox()];
      expect(inner!.x).toBeGreaterThanOrEqual(outer!.x);
      expect(inner!.x + inner!.width).toBeLessThanOrEqual(outer!.x + outer!.width + 0.5);
      return inner!;
    };
    await inside(menu);
    // The name is all there, and not squeezed into a sliver.
    const name = await inside(card.getByText("Local network boost", { exact: true }));
    expect(name.width).toBeGreaterThan(100);
    await inside(card.getByText("when sending"));
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
  });
}

// The same for the other pages that need no camera.
for (const path of ["", "send", "inbox", "devices", "preview"]) {
  test(`no page scrolls sideways on a narrow screen: /${path}`, async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 568 });
    await page.goto(`./#/${path}`);
    await expect(page.getByRole("link", { name: "Receive" }).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBeLessThanOrEqual(0);
  });
}
