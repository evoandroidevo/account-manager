import { expect, test, type Browser, type Page } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";

// Seed users from lldap-bootstrap/.
export const ALICE = { username: "alice", password: "AliceDemo-123!" };
export const BOB = { username: "bob", password: "BobDemo-123!" };

/** Fails a test if the browser logged an error (hydration mismatch, wasm panic). */
export function failOnConsoleErrors() {
  let errors: string[] = [];
  test.beforeEach(({ page }) => {
    errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("console", (message) => {
      // Failed server fns surface as HTTP errors; those are expected in tests.
      if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
        errors.push(message.text());
      }
    });
  });
  test.afterEach(() => {
    expect(errors).toEqual([]);
  });
}

/** Loads a page and waits until the wasm bundle has hydrated it. */
export async function gotoHydrated(page: Page, url: string) {
  await page.goto(url);
  await page.waitForLoadState("networkidle");
}

export async function submitLogin(page: Page, username: string, password: string) {
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Log in" }).click();
}

export async function login(page: Page, username: string, password: string) {
  await gotoHydrated(page, "/login");
  await submitLogin(page, username, password);
  await page.waitForURL("/");
}

/** Creates an invite as bob through the admin page and returns its link. */
export async function createInvite(page: Page, groups: string[] = ["user"]): Promise<string> {
  await login(page, BOB.username, BOB.password);
  await gotoHydrated(page, "/admin/invites");
  for (const group of ["user", "jelly", "admins"]) {
    const box = page.getByRole("checkbox", { name: group, exact: true });
    if (groups.includes(group)) await box.check();
    else await box.uncheck();
  }
  await page.getByRole("button", { name: "Create invite" }).click();
  const link = page.getByTestId("created-invite").getByRole("textbox");
  await expect(link).toHaveValue(/\/invite\?code=[A-Za-z0-9]{32}$/);
  return (await link.inputValue())!;
}

export type NewUser = { username: string; email: string; password: string };

let userCounter = 0;
export function uniqueUser(): NewUser {
  const username = `e2e${Date.now().toString(36)}${userCounter++}`;
  return { username, email: `${username}@example.com`, password: "E2e-Password-123!" };
}

export async function fillSignup(page: Page, user: NewUser, passwordConfirm = user.password) {
  await page.getByLabel("Username").fill(user.username);
  await page.getByLabel("Email").fill(user.email);
  await page.getByLabel("First name").fill("E2E");
  await page.getByLabel("Last name").fill("Tester");
  await page.getByLabel("Password", { exact: true }).fill(user.password);
  await page.getByLabel("Confirm password").fill(passwordConfirm);
  await page.getByRole("button", { name: "Create account" }).click();
}

/** Signs up a brand-new account (via an invite from bob) and returns it. */
export async function signUpNewUser(browser: Browser, groups?: string[]): Promise<NewUser> {
  const adminContext = await browser.newContext();
  const inviteUrl = await createInvite(await adminContext.newPage(), groups);
  await adminContext.close();

  const user = uniqueUser();
  const context = await browser.newContext();
  const page = await context.newPage();
  await gotoHydrated(page, inviteUrl);
  await fillSignup(page, user);
  await page.waitForURL("/");
  await context.close();
  return user;
}

/** The user's LLDAP groups, read straight from the dev LLDAP's API. */
export async function lldapGroups(username: string): Promise<string[]> {
  const lldap = "http://127.0.0.1:17170";
  const login = await fetch(`${lldap}/auth/simple/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: "admin", password: "dev-only-password-change-me" }),
  });
  const { token } = await login.json();
  const response = await fetch(`${lldap}/api/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({
      query: "query ($id: String!) { user(userId: $id) { groups { displayName } } }",
      variables: { id: username },
    }),
  });
  const body = await response.json();
  return body.data.user.groups.map((group: { displayName: string }) => group.displayName).sort();
}

// Emails written by the server's file transport (APP__MAIL__OUTBOX_DIR).
const OUTBOX = path.resolve(__dirname, "../../data/e2e-outbox");

/** Undoes quoted-printable soft line breaks and escapes. */
function decodeQuotedPrintable(text: string): string {
  return text
    .replace(/=\r?\n/g, "")
    .replace(/=([0-9A-F]{2})/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
}

/** Waits for an email to `to` with `subject` and returns its decoded text. */
export async function waitForEmail(to: string, subject: string): Promise<string> {
  let found = "";
  await expect
    .poll(
      () => {
        if (!fs.existsSync(OUTBOX)) return false;
        for (const file of fs.readdirSync(OUTBOX)) {
          const raw = fs.readFileSync(path.join(OUTBOX, file), "utf8");
          if (raw.includes(`To: ${to}`) && raw.includes(`Subject: ${subject}`)) {
            found = decodeQuotedPrintable(raw);
            return true;
          }
        }
        return false;
      },
      { message: `email "${subject}" to ${to}`, timeout: 5000 },
    )
    .toBe(true);
  return found;
}
