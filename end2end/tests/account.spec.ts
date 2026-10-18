import { test, expect } from "@playwright/test";
import {
  failOnConsoleErrors,
  gotoHydrated,
  jellyfinLogin,
  login,
  signUpNewUser,
  submitLogin,
  uniqueUser,
  waitForEmail,
} from "./helpers";

failOnConsoleErrors();

// These tests use freshly signed-up accounts so the seed users never change.

test("changing the password signs out other sessions but keeps this one", async ({
  page,
  browser,
}) => {
  const user = await signUpNewUser(browser);
  const newPassword = "Brand-New-Password-456!";

  const otherDevice = await browser.newPage();
  await login(otherDevice, user.username, user.password);

  await login(page, user.username, user.password);
  await page.getByRole("link", { name: "Password" }).click();
  await expect(page).toHaveURL("/security");
  const alert = page.getByRole("alert");
  const change = async (current: string, next: string, confirm = next) => {
    await page.getByLabel("Current password").fill(current);
    await page.getByLabel("New password", { exact: true }).fill(next);
    await page.getByLabel("Confirm new password").fill(confirm);
    await page.getByRole("button", { name: "Change password" }).click();
  };

  await change("not-my-password", newPassword);
  await expect(alert).toHaveText("Your current password is incorrect.");
  await change(user.password, newPassword, "Mismatched-Password-789!");
  await expect(alert).toHaveText("The new passwords don't match.");
  await change(user.password, "short");
  await expect(alert).toContainText("at least 12 characters");

  await change(user.password, newPassword);
  await expect(page.getByRole("status")).toHaveText(
    "Password changed. You've been signed out of this site on your other devices.",
  );
  await expect(page.getByLabel("Current password")).toHaveValue("");

  // This session survives; the other one is gone. The other device notices
  // on its next page change, without needing a reload.
  await gotoHydrated(page, "/");
  await expect(page).toHaveURL("/");
  await otherDevice.getByRole("link", { name: "Email" }).click();
  await expect(otherDevice).toHaveURL("/login");

  // The new password works and the old one doesn't.
  await submitLogin(otherDevice, user.username, user.password);
  await expect(otherDevice.getByRole("alert")).toHaveText("Invalid username or password.");
  await submitLogin(otherDevice, user.username, newPassword);
  await otherDevice.waitForURL("/");
  await otherDevice.close();

  await waitForEmail(user.email, "Your password was changed");

  // Jellyfin checks the password against LLDAP, so it changed there too.
  // (Polls because Jellyfin sign-in needs the "jelly" group, which the
  // background sync adds.)
  await expect.poll(() => jellyfinLogin(user.username, newPassword), { timeout: 30_000 }).toBe(200);
  expect(await jellyfinLogin(user.username, user.password)).toBe(401);
});

test("changing the email needs the link sent to the new address", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  const newEmail = uniqueUser().email;

  await login(page, user.username, user.password);
  await page.getByRole("link", { name: "Email" }).click();
  await expect(page).toHaveURL("/email");
  await expect(page.getByText(`Your current address is ${user.email}`)).toBeVisible();

  const request = async (email: string, password: string) => {
    await page.getByLabel("New email address").fill(email);
    await page.getByLabel("Current password").fill(password);
    await page.getByRole("button", { name: "Send confirmation link" }).click();
  };

  await request(user.email, user.password);
  await expect(page.getByRole("alert")).toHaveText("That is already your email address.");
  await request(newEmail, "not-my-password");
  await expect(page.getByRole("alert")).toHaveText("Your current password is incorrect.");
  await request(newEmail, user.password);
  await expect(page.getByRole("status")).toHaveText(
    `We sent a confirmation link to ${newEmail}. It expires in 24 hours.`,
  );

  // Nothing changes until the link is used.
  await gotoHydrated(page, "/");
  await expect(page.locator("dl.details")).toContainText(user.email);

  const mail = await waitForEmail(newEmail, "Confirm your new email address");
  const link = mail.match(/http:\/\/127\.0\.0\.1:3000\/confirm-email\?token=[A-Za-z0-9]{32}/)?.[0];
  expect(link).toBeDefined();

  // The link works in any browser, logged in or not, but needs a button press.
  const confirmPage = await browser.newPage();
  await gotoHydrated(confirmPage, link!);
  await expect(confirmPage.getByText(`to ${newEmail}?`)).toBeVisible();
  await confirmPage.getByRole("button", { name: "Confirm change" }).click();
  await expect(confirmPage.getByRole("heading", { name: "Email updated" })).toBeVisible();

  await gotoHydrated(confirmPage, link!);
  await expect(confirmPage.getByRole("heading", { name: "Link not valid" })).toBeVisible();
  await confirmPage.close();

  await gotoHydrated(page, "/");
  await expect(page.locator("dl.details")).toContainText(newEmail);
  await waitForEmail(user.email, "Your email address was changed");
});
