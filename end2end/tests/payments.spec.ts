import { test, expect } from "@playwright/test";
import {
  BOB,
  failOnConsoleErrors,
  gotoHydrated,
  jellyfinUser,
  login,
  MOCK_WHOP_URL,
  mockWhop,
  signedWebhook,
  signUpNewUser,
  uniqueUser,
} from "./helpers";

failOnConsoleErrors();
// Payments then a full LLDAP/Jellyfin/Seerr sync take a while.
test.describe.configure({ timeout: 90_000 });

// Runs against the mock Whop (end2end/mock-whop.mjs), which signs webhooks
// the way Whop does. Plus is bought with plan_plus (end2end/e2e-config.yaml).

const SYNC = { timeout: 30_000 };

test("buying a plan through checkout upgrades the account and its services", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  await login(page, user.username, user.password);

  await page.getByRole("link", { name: "Plan" }).click();
  await expect(page).toHaveURL("/billing");
  const current = page.getByTestId("current-plan");
  await expect(current).toContainText("Your plan: Free");

  // The upgrade button sends the browser to Whop's checkout.
  await page.getByRole("button", { name: "Get Plus" }).click();
  await page.waitForURL(`${MOCK_WHOP_URL}/checkout/**`);
  await expect(page.getByText("Plan: plan_plus")).toBeVisible();

  // Paying sends membership.activated and returns to the plans page, which
  // picks up the new plan on its own.
  await page.getByRole("button", { name: "Pay" }).click();
  await page.waitForURL("/billing?checkout=done");
  await expect(page.getByRole("status")).toContainText("Thanks!");
  await expect(current).toContainText("Your plan: Plus", SYNC);
  await expect(current).toContainText("Paid through Whop, active until");
  const manage = page.getByRole("link", { name: "Manage or cancel your subscription on Whop" });
  await expect(manage).toHaveAttribute("href", new RegExp(`^${MOCK_WHOP_URL}/manage/mem_`));

  // The paid plan reaches Jellyfin like any other plan.
  await expect
    .poll(async () => (await jellyfinUser(user.username))?.Policy?.MaxActiveSessions, SYNC)
    .toBe(3);

  // Cancelling on Whop sends membership.deactivated and ends the paid plan.
  const membershipId = (await manage.getAttribute("href"))!.split("/").pop()!;
  const cancelled = await mockWhop(`/test/memberships/${membershipId}/cancel`);
  expect(cancelled.webhook_status).toBe(200);
  await gotoHydrated(page, "/billing");
  await expect(current).toContainText("Your plan: Free");
  await expect
    .poll(async () => (await jellyfinUser(user.username))?.Policy?.MaxActiveSessions, SYNC)
    .toBe(1);
});

test("webhooks need a valid signature and are processed once", async ({ request }) => {
  const event = { id: `msg_e2e_${Date.now()}`, type: "payment.succeeded", data: {} };

  const forged = signedWebhook(event, "ws_not_the_secret");
  expect((await request.post("/api/webhooks/whop", forged)).status()).toBe(401);
  expect((await request.post("/api/webhooks/whop", { data: "{}" })).status()).toBe(400);

  const genuine = signedWebhook(event);
  const first = await request.post("/api/webhooks/whop", genuine);
  expect(first.status()).toBe(200);
  expect(await first.text()).toBe("ok");
  const retry = await request.post("/api/webhooks/whop", genuine);
  expect(retry.status()).toBe(200);
  expect(await retry.text()).toBe("duplicate");
});

test("a payment that can't be matched waits for an admin to assign it", async ({ page, browser }) => {
  // Bought on Whop directly, with an email no account has.
  const stray = await mockWhop("/test/memberships", {
    plan_id: "plan_plus",
    email: uniqueUser().email,
  });
  expect(stray.webhook_status).toBe(200);
  const user = await signUpNewUser(browser);

  await login(page, BOB.username, BOB.password);
  await page.getByRole("link", { name: "Payments" }).click();
  await expect(page).toHaveURL("/admin/payments");
  const row = page.locator(`tr[data-membership="${stray.id}"]`);
  await expect(row).toContainText("Plus");
  await expect(page.getByTestId("webhook-events")).toContainText("unmatched");

  await row.getByLabel("Username to assign").fill(user.username);
  await row.getByRole("button", { name: "Assign" }).click();
  await expect(row).toHaveCount(0);

  const userPage = await browser.newPage();
  await login(userPage, user.username, user.password);
  await gotoHydrated(userPage, "/billing");
  await expect(userPage.getByTestId("current-plan")).toContainText("Your plan: Plus");
  await userPage.close();
});
