import { devices, defineConfig } from "@playwright/test";

const BASE_URL = "http://127.0.0.1:3000";

/**
 * End-to-end tests against the real server and the dev LLDAP.
 *
 * Prerequisites (from the repo root):
 *   docker compose -f docker-compose.lldap-dev.yml up -d
 *   cargo leptos build
 *
 * The server is started fresh for every run so the in-memory login
 * throttle starts empty. Tests run serially because they share it.
 */
export default defineConfig({
  testDir: "./tests",
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
    },
  ],
  webServer: {
    command: "./target/debug/server",
    cwd: "..",
    url: `${BASE_URL}/health`,
    reuseExistingServer: false,
    timeout: 30 * 1000,
    env: {
      LEPTOS_OUTPUT_NAME: "account-manager",
      LEPTOS_SITE_ROOT: "target/site",
      LEPTOS_SITE_PKG_DIR: "pkg",
      LEPTOS_SITE_ADDR: "127.0.0.1:3000",
      APP_CONFIG: "config.yaml",
      APP__SERVER__PUBLIC_URL: BASE_URL,
      APP__SESSION__SECURE_COOKIE: "false",
      APP__AUTH__LOGIN_MAX_ATTEMPTS: "5",
    },
  },
});
