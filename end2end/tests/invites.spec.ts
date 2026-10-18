import { test, expect } from "@playwright/test";
import {
  ALICE,
  BOB,
  createInvite,
  failOnConsoleErrors,
  fillSignup,
  gotoHydrated,
  lldapGroups,
  login,
  uniqueUser,
} from "./helpers";

failOnConsoleErrors();

test("an admin invite lets a new user sign up exactly once", async ({ page, browser }) => {
  const inviteUrl = await createInvite(page, ["user", "jelly"]);

  const guest = await browser.newPage();
  await gotoHydrated(guest, inviteUrl);
  await expect(guest.getByRole("heading", { name: "Create your account" })).toBeVisible();

  // The two-column name row stays inside the card.
  const card = (await guest.locator("section.card").boundingBox())!;
  const lastName = (await guest.getByLabel("Last name").boundingBox())!;
  expect(lastName.x + lastName.width).toBeLessThanOrEqual(card.x + card.width);

  const alert = guest.getByRole("alert");
  const user = uniqueUser();

  // Validation errors keep the visitor on the form.
  await fillSignup(guest, user, "Something-Else-123!");
  await expect(alert).toHaveText("The passwords don't match.");
  await fillSignup(guest, { ...user, username: ALICE.username });
  await expect(alert).toHaveText("That username is already taken.");

  // A valid form creates the LLDAP account and logs the new user straight in.
  await fillSignup(guest, user);
  await guest.waitForURL("/");
  await expect(guest.getByRole("heading", { name: "E2E Tester" })).toBeVisible();
  await expect(guest.locator("dl.details")).toContainText(user.email);
  await guest.close();
  expect(await lldapGroups(user.username)).toEqual(["jelly", "user"]);

  // The link is single-use.
  const second = await browser.newPage();
  await gotoHydrated(second, inviteUrl);
  await expect(second.getByRole("heading", { name: "Invitation not valid" })).toBeVisible();
  await second.close();

  // The admin list shows who used it.
  await page.reload();
  await page.waitForLoadState("networkidle");
  const row = page.locator("table.invites tbody tr").first();
  await expect(row).toContainText("used");
  await expect(row).toContainText(`by ${user.username}`);
});

test("a revoked invite can no longer be used", async ({ page, browser }) => {
  const inviteUrl = await createInvite(page);

  const row = page.locator("table.invites tbody tr").first();
  await expect(row).toContainText("active");
  await row.getByRole("button", { name: "Revoke" }).click();
  await expect(row).toContainText("revoked");

  const guest = await browser.newPage();
  await gotoHydrated(guest, inviteUrl);
  await expect(guest.getByRole("heading", { name: "Invitation not valid" })).toBeVisible();
  await guest.close();
});

test("opening an invite while logged in asks to log out instead of switching accounts", async ({
  page,
}) => {
  const inviteUrl = await createInvite(page);

  // bob is still logged in on this page.
  await gotoHydrated(page, inviteUrl);
  await expect(page.getByRole("heading", { name: "You're already logged in" })).toBeVisible();
  await expect(page.getByText("You're logged in as bob.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Create account" })).toHaveCount(0);

  await page.getByRole("button", { name: "Log out and continue" }).click();
  await expect(page.getByRole("heading", { name: "Create your account" })).toBeVisible();
  await expect(page).toHaveURL(inviteUrl);

  // bob's session really ended.
  await gotoHydrated(page, "/");
  await expect(page).toHaveURL("/login");
});

test("made-up invite codes are rejected", async ({ page }) => {
  await gotoHydrated(page, "/invite?code=notarealcode");
  await expect(page.getByRole("heading", { name: "Invitation not valid" })).toBeVisible();
});

test("non-admins can't see or use the invites page", async ({ page }) => {
  await login(page, ALICE.username, ALICE.password);
  await expect(page.getByRole("link", { name: "Invites" })).toHaveCount(0);

  await gotoHydrated(page, "/admin/invites");
  await expect(page.getByText("Only administrators can do that.").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Create invite" })).toHaveCount(0);
});

test("admins see the invites link", async ({ page }) => {
  await login(page, BOB.username, BOB.password);
  await page.getByRole("link", { name: "Invites" }).click();
  await expect(page).toHaveURL("/admin/invites");
  await expect(page.getByRole("heading", { name: "Create an invite" })).toBeVisible();

  // Only regular groups are offered; LLDAP's own permission groups are hidden.
  const groups = page.getByRole("group", { name: "Groups the new account joins" });
  await expect(groups.getByRole("checkbox")).not.toHaveCount(0);
  for (const name of await groups.getByRole("checkbox").evaluateAll((boxes) =>
    boxes.map((box) => box.parentElement?.textContent?.trim() ?? ""),
  )) {
    expect(name).not.toMatch(/^lldap_/);
  }
  await expect(groups.getByRole("checkbox", { name: "lldap_admin" })).toHaveCount(0);
});
