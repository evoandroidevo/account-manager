import { test, expect, type Page } from "@playwright/test";
import {
  failOnConsoleErrors,
  gotoHydrated,
  jellyfinUser,
  login,
  MOCK_WHOP_URL,
  mockWhop,
  mockWhopGet,
  signUpNewUser,
} from "./helpers";

failOnConsoleErrors();
test.describe.configure({ timeout: 120_000 });

// Plus costs $5 and Premium $15 a month on the mock Whop
// (end2end/mock-whop.mjs, end2end/e2e-config.yaml).

const SYNC = { timeout: 30_000 };

/** Pays on the mock checkout and waits for the plans page to show `plan`. */
async function pay(page: Page, plan: string) {
  await page.waitForURL(`${MOCK_WHOP_URL}/checkout/**`);
  await page.getByRole("button", { name: "Pay" }).click();
  await page.waitForURL("/billing?checkout=done");
  await expect(page.getByTestId("current-plan")).toContainText(`Your plan: ${plan}`, SYNC);
}

/** The id of the user's current Whop membership, from the manage link. */
async function membershipId(page: Page) {
  const manage = page.getByRole("link", { name: "Manage or cancel your subscription on Whop" });
  return (await manage.getAttribute("href"))!.split("/").pop()!;
}

const card = (page: Page, tier: string) => page.locator(`[data-tier="${tier}"]`);

test("upgrading takes the unused part of the current plan off the new one", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  await login(page, user.username, user.password);
  await gotoHydrated(page, "/billing");
  await page.getByRole("button", { name: "Get Plus" }).click();
  await pay(page, "Plus");
  const plus = await membershipId(page);

  // Half of the month is used, so half of the $5 comes back.
  expect((await mockWhop(`/test/memberships/${plus}/halfway`)).webhook_status).toBe(200);
  await gotoHydrated(page, "/billing");
  await expect(card(page, "premium").getByTestId("upgrade-quote")).toHaveText(
    /^\$12\.5[01] today \(\$15\.00 minus \$2\.(49|50) for the unused part of Plus\)$/,
  );
  await expect(card(page, "free").getByRole("button")).toHaveText(/^Switch to Free on /);

  // The checkout charges the reduced amount.
  await card(page, "premium").getByRole("button", { name: "Upgrade to Premium" }).click();
  await page.waitForURL(`${MOCK_WHOP_URL}/checkout/**?promoCode=up*`);
  await expect(page.getByTestId("amount-due")).toHaveText(/^Due today: \$12\.5[01]$/);
  await pay(page, "Premium");

  // The old membership is cancelled straight away, so it isn't billed again.
  await expect.poll(async () => (await mockWhopGet(`/test/memberships/${plus}`)).status, SYNC).toBe("canceled");
  const promos = await mockWhopGet("/test/promo_codes");
  const promo = promos.find((code: { plan_ids: string[]; uses: number }) => code.uses > 0);
  expect(promo).toMatchObject({ plan_ids: ["plan_premium"], stock: 1, uses: 1 });
  await expect
    .poll(async () => (await jellyfinUser(user.username))?.Policy?.MaxActiveSessions, SYNC)
    .toBe(5);
});

test("downgrading waits for the end of the paid period and can be undone", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  await login(page, user.username, user.password);
  await gotoHydrated(page, "/billing");
  await page.getByRole("button", { name: "Get Premium" }).click();
  await pay(page, "Premium");
  const premium = await membershipId(page);
  const current = page.getByTestId("current-plan");
  const notice = page.getByTestId("pending-change");

  // Scheduling it stops the renewal; Premium stays until the period ends.
  await card(page, "plus").getByRole("button", { name: /^Switch to Plus on / }).click();
  await expect(notice).toContainText("You'll move to Plus on");
  await expect(card(page, "plus")).toContainText("Starts");
  await expect(current).toContainText("Your plan: Premium");
  expect((await mockWhopGet(`/test/memberships/${premium}`)).cancel_at_period_end).toBe(true);

  // Keeping Premium renews it again.
  await notice.getByRole("button", { name: "Keep Premium" }).click();
  await expect(notice).toHaveCount(0);
  await expect(card(page, "plus").getByRole("button", { name: /^Switch to Plus on / })).toBeVisible();
  expect((await mockWhopGet(`/test/memberships/${premium}`)).cancel_at_period_end).toBe(false);

  // Schedule it again and let the period end: the user is offered Plus, at
  // full price since nothing is left to credit.
  await card(page, "plus").getByRole("button", { name: /^Switch to Plus on / }).click();
  await expect(notice).toContainText("You'll move to Plus on");
  expect((await mockWhop(`/test/memberships/${premium}/cancel`)).webhook_status).toBe(200);
  await gotoHydrated(page, "/billing");
  await expect(current).toContainText("Your plan: Free");
  await expect(notice).toContainText("Your previous plan has ended");
  await notice.getByRole("button", { name: "Continue with Plus" }).click();
  await page.waitForURL(`${MOCK_WHOP_URL}/checkout/**`);
  await expect(page.getByTestId("amount-due")).toHaveText("Due today: $5.00");
  await pay(page, "Plus");
  await expect(notice).toHaveCount(0);
});
