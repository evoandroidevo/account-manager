import { DatabaseSync } from "node:sqlite";
import { test, expect } from "@playwright/test";
import { ALICE, BOB, failOnConsoleErrors, gotoHydrated, login, submitLogin } from "./helpers";

failOnConsoleErrors();

test("logged-out visitors are sent to the login page", async ({ page }) => {
  for (const path of ["/", "/settings", "/security", "/email", "/admin/invites"]) {
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

test("a username that can't exist isn't written to the audit log", async ({ page, request }) => {
  await page.goto("/login");
  const action = await page.locator("form").getAttribute("action");
  const username = `longprobe${"a".repeat(100_000)}`;

  const response = await request.post(action!, { form: { username, password: "x" } });
  expect(await response.text()).toContain("Invalid username or password.");

  const db = new DatabaseSync("../data/e2e.db", { readOnly: true });
  const rows = db.prepare("SELECT count(*) AS n FROM audit_log WHERE target LIKE 'longprobe%'").get();
  db.close();
  expect(rows).toMatchObject({ n: 0 });
});

test("failures from one IP don't lock the user out elsewhere, but many IPs do", async ({ page, request }) => {
  await page.goto("/login");
  const action = (await page.locator("form").getAttribute("action"))!;
  const attempt = async (ip: string, username: string, password: string) =>
    (
      await request.post(action, {
        headers: { "X-Forwarded-For": ip },
        form: { username, password },
        maxRedirects: 0,
      })
    ).text();
  const BLOCKED = "Too many failed attempts";

  // The attacker's IP gets blocked; Alice's own still works.
  for (let i = 0; i < 5; i++) await attempt("203.0.113.1", ALICE.username, "wrong");
  expect(await attempt("203.0.113.1", ALICE.username, ALICE.password)).toContain(BLOCKED);
  expect(await attempt("203.0.113.2", ALICE.username, ALICE.password)).not.toContain(BLOCKED);

  // Guessing one username from many IPs hits the per-username limit (10 × 5).
  for (let i = 0; i < 50; i++) await attempt(`198.51.100.${i}`, "distributedtarget", "wrong");
  expect(await attempt("198.51.100.200", "distributedtarget", "wrong")).toContain(BLOCKED);
});

test("logging in to your own account doesn't reset the IP's failure count", async ({ page, request }) => {
  await page.goto("/login");
  const action = (await page.locator("form").getAttribute("action"))!;
  const attempt = async (username: string, password: string) =>
    (
      await request.post(action, {
        headers: { "X-Forwarded-For": "203.0.113.3" },
        form: { username, password },
        maxRedirects: 0,
      })
    ).text();

  for (let i = 0; i < 4; i++) await attempt("guessedtarget", "wrong");
  expect(await attempt(BOB.username, BOB.password)).not.toContain("Too many failed attempts");
  await attempt("guessedtarget", "wrong");
  expect(await attempt("guessedtarget", "wrong")).toContain("Too many failed attempts");
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
