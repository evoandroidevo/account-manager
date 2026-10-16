import { test, expect, type Page } from "@playwright/test";

// Seed users from lldap-bootstrap/.
const ALICE = { username: "alice", password: "AliceDemo-123!" };
const BOB = { username: "bob", password: "BobDemo-123!" };

// Tests share the server's in-memory login throttle, so order matters:
// the throttling test runs last.
test.describe.configure({ mode: "serial" });

let consoleErrors: string[] = [];

test.beforeEach(({ page }) => {
  consoleErrors = [];
  page.on("pageerror", (error) => consoleErrors.push(error.message));
  page.on("console", (message) => {
    // Failed server fns surface as HTTP errors; those are expected here.
    if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
      consoleErrors.push(message.text());
    }
  });
});

test.afterEach(() => {
  // Catches hydration mismatches and wasm panics.
  expect(consoleErrors).toEqual([]);
});

/** Loads a page and waits until the wasm bundle has hydrated it. */
async function gotoHydrated(page: Page, path: string) {
  await page.goto(path);
  await page.waitForLoadState("networkidle");
}

async function submitLogin(page: Page, username: string, password: string) {
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Log in" }).click();
}

async function login(page: Page, username: string, password: string) {
  await gotoHydrated(page, "/login");
  await submitLogin(page, username, password);
  await page.waitForURL("/");
}

test("logged-out visitors are sent to the login page", async ({ page }) => {
  await gotoHydrated(page, "/");
  await expect(page).toHaveURL("/login");
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
  const details = page.locator("dl.details");
  await expect(details).toContainText("alice@example.com");
  await expect(page.locator("ul.chips li")).toHaveText(["jelly", "user"]);
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

// Must stay last: it blocks this IP for the rest of the run.
test("repeated failures are throttled per IP, even for the right password", async ({ page }) => {
  await gotoHydrated(page, "/login");
  const alert = page.getByRole("alert");

  // Waits for each server fn round trip, since the previous error stays on screen.
  const attempt = async (username: string, password: string) => {
    const response = page.waitForResponse((r) => r.url().includes("/api/login"));
    await submitLogin(page, username, password);
    await response;
    // The action's value and pending flag update together once the body is read.
    await expect(page.getByRole("button", { name: "Log in", exact: true })).toBeEnabled();
    return (await alert.textContent()) ?? "";
  };

  let message = "";
  for (let i = 0; i < 6 && !message.startsWith("Too many failed attempts"); i++) {
    message = await attempt("mallory", `wrong-${i}`);
  }
  expect(message).toMatch(/^Too many failed attempts/);

  // A different username from the same IP is blocked too.
  expect(await attempt(ALICE.username, ALICE.password)).toMatch(/^Too many failed attempts/);
  await expect(page).toHaveURL("/login");
});
