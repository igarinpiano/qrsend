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
