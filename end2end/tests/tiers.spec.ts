import { test, expect, type Page } from "@playwright/test";
import {
  ALICE,
  BOB,
  failOnConsoleErrors,
  gotoHydrated,
  lldapGroups,
  login,
  signUpNewUser,
} from "./helpers";

failOnConsoleErrors();
// Plan changes are applied to LLDAP, Jellyfin and Seerr in the background.
test.describe.configure({ timeout: 90_000 });

/** YYYY-MM-DD, `days` from today (UTC). */
function dateInDays(days: number): string {
  return new Date(Date.now() + days * 86_400_000).toISOString().slice(0, 10);
}

async function openEditor(page: Page, username: string) {
  const row = page.locator(`tr[data-username="${username}"]`);
  await row.getByRole("button", { name: "Edit" }).click();
  return row;
}

/** Waits for the background worker to bring the user's groups in line. */
async function expectGroups(username: string, groups: string[]) {
  await expect
    .poll(() => lldapGroups(username), { timeout: 20_000, message: `${username}'s groups` })
    .toEqual(groups);
}

test("an admin can put a user on another plan and take them off it", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  await expectGroups(user.username, ["jelly", "user"]);

  // The new user starts on the default plan.
  const userPage = await browser.newPage();
  await login(userPage, user.username, user.password);
  await expect(userPage.getByTestId("plan")).toContainText("Free");

  // bob moves them to Plus until a date.
  await login(page, BOB.username, BOB.password);
  await page.getByRole("link", { name: "Users" }).click();
  await expect(page).toHaveURL("/admin/users");

  // Every cell in a row ends at the same height (a flex <td> once broke this).
  await expect(page.locator(`tr[data-username="${user.username}"]`)).toBeVisible();
  const cellBottoms = await page.locator(`tr[data-username="${user.username}"] > td`).evaluateAll(
    (cells) => cells.map((cell) => Math.round(cell.getBoundingClientRect().bottom)),
  );
  expect(new Set(cellBottoms).size).toBe(1);

  const row = await openEditor(page, user.username);
  const until = dateInDays(30);
  await page.getByLabel(`Plan for ${user.username}`).selectOption({ label: "Plus" });
  await page.getByLabel("Ends (optional)").fill(until);
  await page.getByLabel("Note").fill("e2e trial");
  await page.getByRole("button", { name: "Save plan" }).click();
  await expect(row).toContainText("Plus");
  await expect(row).toContainText(`set by admin, ends ${until}`);

  // The worker adds the tier's extra group, and every service shows as synced.
  await expectGroups(user.username, ["jelly", "plus", "user"]);
  await expect
    .poll(async () => {
      await page.getByRole("button", { name: "Refresh" }).click();
      return row.locator(".status-ok").allTextContents();
    }, { timeout: 30_000 })
    .toEqual(["Groups", "Jellyfin", "Seerr"]);

  // The user sees their new plan and when it ends.
  await gotoHydrated(userPage, "/");
  await expect(userPage.getByTestId("plan")).toContainText("Plus");
  await expect(userPage.getByTestId("plan")).toContainText(`Ends ${until}`);

  // Removing the override puts them back on the default plan.
  await openEditor(page, user.username);
  await page.getByRole("button", { name: "Remove override" }).click();
  await expect(row).toContainText("Free");
  await expect(row).toContainText("standard");
  await expectGroups(user.username, ["jelly", "user"]);

  await gotoHydrated(userPage, "/");
  await expect(userPage.getByTestId("plan")).toContainText("Free");
  await userPage.close();
});

test("plan end dates must be in the future", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  await login(page, BOB.username, BOB.password);
  await gotoHydrated(page, "/admin/users");

  await openEditor(page, user.username);
  await page.getByLabel(`Plan for ${user.username}`).selectOption({ label: "Plus" });
  await page.getByLabel("Ends (optional)").fill(dateInDays(-1));
  await page.getByRole("button", { name: "Save plan" }).click();
  await expect(page.getByRole("alert")).toHaveText("The end date must be in the future.");
  await expect(page.locator(`tr[data-username="${user.username}"]`)).toContainText("Free");
});

test("non-admins can't see or use the users page", async ({ page }) => {
  await login(page, ALICE.username, ALICE.password);
  await expect(page.getByRole("link", { name: "Users" })).toHaveCount(0);

  await gotoHydrated(page, "/admin/users");
  await expect(page.getByText("Only administrators can do that.").first()).toBeVisible();
  await expect(page.locator("table.users")).toHaveCount(0);
});
