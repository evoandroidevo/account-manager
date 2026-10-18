import { test, expect } from "@playwright/test";
import { ALICE, failOnConsoleErrors, gotoHydrated, submitLogin } from "./helpers";

failOnConsoleErrors();

// Runs in its own project after every other test: it blocks this IP.
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
