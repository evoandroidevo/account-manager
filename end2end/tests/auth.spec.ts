import { test, expect } from "@playwright/test";
import { ALICE, BOB, failOnConsoleErrors, gotoHydrated, login, submitLogin } from "./helpers";

failOnConsoleErrors();

test("logged-out visitors are sent to the login page", async ({ page }) => {
  for (const path of ["/", "/security", "/email", "/admin/invites"]) {
    await gotoHydrated(page, path);
    await expect(page).toHaveURL("/login");
  }
  await expect(page.getByRole("heading", { name: "Log in" })).toBeVisible();
});

test("a wrong password shows an error without reloading the page", async ({ page }) => {
  await gotoHydrated(page, "/login");
  await submitLogin(page, ALICE.username, "definitely-wrong");

  await expect(page.getByRole("alert")).toHaveText("Invalid username or password.");
  // The hydrated form submits via fetch; the no-JS fallback would land on ?__err=...
  await expect(page).toHaveURL("/login");
});

test("logging in shows the profile from LLDAP and survives a reload", async ({ page, context }) => {
  await login(page, ALICE.username, ALICE.password);

  await expect(page.getByRole("heading", { name: "Alice Example" })).toBeVisible();
  await expect(page.locator("dl.details")).toContainText("alice@example.com");
  await expect(page.getByText("Groups")).toHaveCount(0);
  await expect(page.locator(".badge")).toHaveCount(0);

  const cookie = (await context.cookies()).find((c) => c.name === "am_session");
  expect(cookie).toBeDefined();
  expect(cookie!.httpOnly).toBe(true);
  expect(cookie!.sameSite).toBe("Lax");

  await page.reload();
  await expect(page.getByRole("heading", { name: "Alice Example" })).toBeVisible();
});

test("usernames are case-insensitive", async ({ page }) => {
  await login(page, "ALICE", ALICE.password);
  await expect(page.getByRole("heading", { name: "Alice Example" })).toBeVisible();
});

test("members of the admin group get the admin badge", async ({ page }) => {
  await login(page, BOB.username, BOB.password);
  await expect(page.getByRole("heading", { name: "Bob Admin" })).toBeVisible();
  await expect(page.locator(".badge")).toHaveText("admin");
});

test("logging out ends the session", async ({ page }) => {
  await login(page, ALICE.username, ALICE.password);

  await page.getByRole("button", { name: "Log out" }).click();
  await page.waitForURL("/login");

  await gotoHydrated(page, "/");
  await expect(page).toHaveURL("/login");
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("the login form still works as a plain HTML form", async ({ page }) => {
    await page.goto("/login");
    await submitLogin(page, ALICE.username, "definitely-wrong");
    await expect(page.getByRole("alert")).toHaveText("Invalid username or password.");

    await submitLogin(page, ALICE.username, ALICE.password);
    await page.waitForURL("/");
    await expect(page.getByRole("heading", { name: "Alice Example" })).toBeVisible();
  });
});

test("cross-origin form posts are refused", async ({ page, request }) => {
  await page.goto("/login");
  const action = await page.locator("form").getAttribute("action");
  expect(action).toMatch(/^\/api\/login/);

  const response = await request.post(action!, {
    headers: { Origin: "https://evil.example", Accept: "text/html" },
    form: { username: ALICE.username, password: ALICE.password },
    maxRedirects: 0,
  });
  expect(response.status()).toBe(403);
});
