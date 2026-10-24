import { devices, defineConfig } from "@playwright/test";

const BASE_URL = "http://127.0.0.1:3000";
const MOCK_WHOP_URL = "http://127.0.0.1:3011";
// Test-only Whop credentials; tests/helpers.ts has the same values.
const WHOP = { apiKey: "test-whop-key", webhookSecret: "ws_test_secret" };

/**
 * End-to-end tests against the real server and the dev LLDAP.
 *
 * Prerequisites (from the repo root):
 *   docker compose -f docker-compose.lldap-dev.yml up -d   # LLDAP, Jellyfin, Seerr
 *   cargo leptos build
 *
 * The server is started fresh for every run so the in-memory login
 * throttle starts empty. Tests run serially because they share it.
 */
export default defineConfig({
  testDir: "./tests",
  // Deletes the e2e accounts from LLDAP, Jellyfin and Seerr afterwards.
  globalTeardown: "./cleanup.mjs",
  timeout: 30 * 1000,
  expect: { timeout: 5000 },
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
      testIgnore: /throttle\.spec\.ts/,
    },
    {
      // Blocks the test client's IP, so it runs after everything else.
      name: "throttle",
      use: { ...devices["Desktop Chrome"] },
      testMatch: /throttle\.spec\.ts/,
      dependencies: ["chromium"],
    },
  ],
  webServer: [
    {
      // Stand-in for Whop's API and checkout (see mock-whop.mjs).
      command: "node end2end/mock-whop.mjs",
      cwd: "..",
      url: `${MOCK_WHOP_URL}/health`,
      reuseExistingServer: false,
      env: {
        MOCK_WHOP_PORT: "3011",
        MOCK_WHOP_API_KEY: WHOP.apiKey,
        MOCK_WHOP_WEBHOOK_SECRET: WHOP.webhookSecret,
        MOCK_WHOP_WEBHOOK_URL: `${BASE_URL}/api/webhooks/whop`,
      },
    },
    {
      // Sets up the dev Jellyfin and Seerr, then starts target/debug/server.
      command: "node end2end/start-server.mjs",
      cwd: "..",
      url: `${BASE_URL}/health`,
      reuseExistingServer: false,
      // Show server logs (errors behind "Something went wrong") in the test output.
      stdout: "pipe",
      stderr: "pipe",
      // First run includes the Jellyfin and Seerr setup wizards.
      timeout: 120 * 1000,
      env: {
        LEPTOS_OUTPUT_NAME: "account-manager",
        LEPTOS_SITE_ROOT: "target/site",
        LEPTOS_SITE_PKG_DIR: "pkg",
        LEPTOS_SITE_ADDR: "127.0.0.1:3000",
        // Its own config (tiers), never the developer's config.yaml, so local
        // settings can't change the results.
        APP_CONFIG: "end2end/e2e-config.yaml",
        APP__DATABASE__URL: "sqlite://./data/e2e.db",
        APP__SERVER__PUBLIC_URL: BASE_URL,
        APP__SESSION__SECURE_COOKIE: "false",
        APP__AUTH__LOGIN_MAX_ATTEMPTS: "5",
        APP__MAIL__TRANSPORT: "file",
        APP__MAIL__OUTBOX_DIR: "data/e2e-outbox",
        APP__MAIL__FROM: "Account Manager <no-reply@example.com>",
        APP__WHOP__ENABLED: "true",
        APP__WHOP__API_URL: MOCK_WHOP_URL,
        APP__WHOP__API_KEY: WHOP.apiKey,
        APP__WHOP__ACCOUNT_ID: "biz_e2e",
        APP__WHOP__WEBHOOK_SECRET: WHOP.webhookSecret,
      },
    },
  ],
});
