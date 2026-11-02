import { DatabaseSync } from "node:sqlite";
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
  waitForEmails,
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

  const db = new DatabaseSync("../data/e2e.db", { readOnly: true });
  const row = db.prepare("SELECT verified_email FROM users WHERE username = ?").get(user.username);
  db.close();
  expect(row).toMatchObject({ verified_email: newEmail });

  await gotoHydrated(confirmPage, link!);
  await expect(confirmPage.getByRole("heading", { name: "Link not valid" })).toBeVisible();
  await confirmPage.close();

  await gotoHydrated(page, "/");
  await expect(page.locator("dl.details")).toContainText(newEmail);
  await waitForEmail(user.email, "Your email address was changed");
});

test("signing up mails a link that verifies the address", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);

  const mail = await waitForEmail(user.email, "Verify your email address");
  const link = mail.match(/http:\/\/127\.0\.0\.1:3000\/confirm-email\?token=[A-Za-z0-9]{32}/)?.[0];
  expect(link).toBeDefined();

  await gotoHydrated(page, link!);
  await page.getByRole("button", { name: "Confirm change" }).click();
  await expect(page.getByRole("heading", { name: "Email updated" })).toBeVisible();

  // Waits out the server's writes instead of failing with "database is locked".
  const db = new DatabaseSync("../data/e2e.db", { readOnly: true, timeout: 5000 });
  const verified = db.prepare("SELECT verified_email FROM users WHERE username = ?").get(user.username);
  const audit = db
    .prepare("SELECT action FROM audit_log WHERE actor = ? AND action LIKE 'email_%'")
    .all(user.username);
  db.close();
  expect(verified).toMatchObject({ verified_email: user.email });
  expect(audit).toEqual([{ action: "email_verified" }]);
});

test("the account page sends new verification links until the address is verified", async ({
  page,
  browser,
}) => {
  const user = await signUpNewUser(browser);
  const subject = "Verify your email address";
  const linkIn = (mail: string) =>
    mail.match(/http:\/\/127\.0\.0\.1:3000\/confirm-email\?token=[A-Za-z0-9]{32}/)![0];

  await login(page, user.username, user.password);
  await expect(page.locator("dl.details")).toContainText("Not verified");
  const send = page.getByRole("button", { name: "Send verification link" });

  // Three an hour, on top of the one from signup.
  for (let sent = 1; sent <= 3; sent++) {
    await send.click();
    await waitForEmails(user.email, subject, sent + 1);
  }
  await expect(page.getByRole("status")).toHaveText(
    `We sent a verification link to ${user.email}. It expires in 24 hours.`,
  );
  await send.click();
  await expect(page.getByRole("alert")).toContainText("We've sent several links recently.");

  // Only the newest link works.
  const mails = await waitForEmails(user.email, subject, 4);
  await gotoHydrated(page, linkIn(mails[0]));
  await expect(page.getByRole("heading", { name: "Link not valid" })).toBeVisible();
  await gotoHydrated(page, linkIn(mails[3]));
  await page.getByRole("button", { name: "Confirm change" }).click();
  await expect(page.getByRole("heading", { name: "Email updated" })).toBeVisible();

  await gotoHydrated(page, "/");
  await expect(page.locator("dl.details")).toContainText(user.email);
  await expect(page.locator("dl.details")).not.toContainText("Not verified");
  await expect(send).toHaveCount(0);
});
