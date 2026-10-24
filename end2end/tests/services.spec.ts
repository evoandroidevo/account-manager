import { test, expect, type Page } from "@playwright/test";
import {
  BOB,
  failOnConsoleErrors,
  gotoHydrated,
  jellyfinLibraryId,
  jellyfinLogin,
  jellyfinUser,
  login,
  seerrQuotas,
  seerrUser,
  signUpNewUser,
} from "./helpers";

failOnConsoleErrors();
// Signup plus a full Jellyfin and Seerr sync takes longer than the default.
test.describe.configure({ timeout: 90_000 });

// These run against the real Jellyfin and Seerr containers from
// docker-compose.lldap-dev.yml. Plans come from end2end/e2e-config.yaml.

const SYNC = { timeout: 30_000 };
const REQUEST = 32;
const AUTO_APPROVE = 128;

/** Waits until the user exists in Jellyfin and Seerr; returns both records. */
async function provisioned(username: string) {
  let jellyfin: any;
  await expect.poll(async () => (jellyfin = await jellyfinUser(username)), SYNC).toBeTruthy();
  let seerr: any;
  await expect.poll(async () => (seerr = await seerrUser(username, jellyfin.Id)), SYNC).toBeTruthy();
  return { jellyfin, seerr };
}

async function setPlan(page: Page, username: string, plan: "Plus" | null) {
  await login(page, BOB.username, BOB.password);
  await gotoHydrated(page, "/admin/users");
  const row = page.locator(`tr[data-username="${username}"]`);
  await row.getByRole("button", { name: "Edit" }).click();
  if (plan) {
    await page.getByLabel(`Plan for ${username}`).selectOption({ label: plan });
    await page.getByRole("button", { name: "Save plan" }).click();
    await expect(row).toContainText(plan);
  } else {
    await page.getByRole("button", { name: "Remove override" }).click();
    await expect(row).toContainText("standard");
  }
}

test("new accounts are created in Jellyfin and Seerr on the default plan", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  const { jellyfin, seerr } = await provisioned(user.username);

  // The account is created first and the plan's policy applied right after.
  await expect.poll(async () => (await jellyfinUser(user.username)).Policy, SYNC).toMatchObject({
    IsAdministrator: false,
    MaxActiveSessions: 1,
    RemoteClientBitrateLimit: 8_000_000,
    EnableVideoPlaybackTranscoding: true,
    EnableContentDownloading: false,
    EnableAllFolders: true,
    // Free has no Live TV; nobody manages recordings; global lockout applies.
    EnableLiveTvAccess: false,
    EnableLiveTvManagement: false,
    LoginAttemptsBeforeLockout: 5,
    AuthenticationProviderId: "Jellyfin.Plugin.LDAP_Auth.LdapAuthenticationProviderPlugin",
  });

  // One password everywhere: the new user signs in to Jellyfin with their
  // LLDAP password through the LDAP plugin.
  expect(await jellyfinLogin(user.username, user.password)).toBe(200);
  expect(await jellyfinLogin(user.username, "not-the-password")).toBe(401);
  await expect.poll(async () => (await seerrUser(user.username, jellyfin.Id)).permissions, SYNC).toBe(REQUEST);
  await expect.poll(() => seerrQuotas(seerr.id), SYNC).toEqual({ movie: [2, 7], tv: [1, 7] });

  // The user sees both services as ready.
  await login(page, user.username, user.password);
  const plan = page.getByTestId("plan");
  await expect
    .poll(async () => {
      await gotoHydrated(page, "/");
      return plan.locator("li").allInnerTexts();
    }, SYNC)
    .toEqual(["Jellyfin\nReady", "Seerr\nReady"]);
});

test("changing the plan updates the Jellyfin policy and Seerr limits", async ({ page, browser }) => {
  const user = await signUpNewUser(browser);
  const { jellyfin, seerr } = await provisioned(user.username);
  const movies = await jellyfinLibraryId("Movies");

  await setPlan(page, user.username, "Plus");
  await expect
    .poll(async () => (await jellyfinUser(user.username)).Policy, SYNC)
    .toMatchObject({
      MaxActiveSessions: 3,
      RemoteClientBitrateLimit: 0,
      EnableContentDownloading: true,
      EnableAllFolders: false,
      EnabledFolders: [movies],
      EnableLiveTvAccess: true,
      EnableLiveTvManagement: false,
      LoginAttemptsBeforeLockout: 5,
    });
  await expect
    .poll(async () => (await seerrUser(user.username, jellyfin.Id)).permissions, SYNC)
    .toBe(REQUEST | AUTO_APPROVE);
  await expect.poll(() => seerrQuotas(seerr.id), SYNC).toEqual({ movie: [10, 7], tv: [5, 7] });

  await setPlan(page, user.username, null);
  await expect
    .poll(async () => (await jellyfinUser(user.username)).Policy, SYNC)
    .toMatchObject({
      MaxActiveSessions: 1,
      EnableContentDownloading: false,
      EnableAllFolders: true,
      EnableLiveTvAccess: false,
    });
  await expect
    .poll(async () => (await seerrUser(user.username, jellyfin.Id)).permissions, SYNC)
    .toBe(REQUEST);
  await expect.poll(() => seerrQuotas(seerr.id), SYNC).toEqual({ movie: [2, 7], tv: [1, 7] });
});

test("Jellyfin and Seerr administrators are never changed", async ({ page, browser }) => {
  // LLDAP's "admin" is also the Jellyfin administrator and Seerr's owner in
  // the dev containers. Logging in queues a sync for it.
  const adminPage = await browser.newPage();
  await login(adminPage, "admin", "dev-only-password-change-me");
  await adminPage.close();

  await login(page, BOB.username, BOB.password);
  await gotoHydrated(page, "/admin/users");
  const row = page.locator('tr[data-username="admin"]');
  await expect
    .poll(async () => {
      await page.getByRole("button", { name: "Refresh" }).click();
      return row.locator(".status-skipped").allTextContents();
    }, SYNC)
    .toEqual(["Jellyfin", "Seerr"]);

  const jellyfin = await jellyfinUser("admin");
  expect(jellyfin.Policy.IsAdministrator).toBe(true);
  expect(jellyfin.Policy.MaxActiveSessions).toBe(0);
  const seerr = await seerrUser("admin", jellyfin.Id);
  expect(seerr.id).toBe(1);
  expect(seerr.permissions).toBe(2);
});
