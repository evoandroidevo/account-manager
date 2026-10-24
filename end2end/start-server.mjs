// Playwright's webServer command (run from the repo root). Sets up the dev
// Jellyfin and Seerr first, then starts the account manager pointed at them.
// Playwright starts web servers before globalSetup, so this can't live there.

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";

import { setupServices } from "./services.mjs";

const { jellyfin, seerr } = await setupServices();

// Start from an empty database: the previous run's accounts were deleted
// from LLDAP and the services by cleanup.mjs.
for (const suffix of ["", "-wal", "-shm"]) {
  rmSync(`data/e2e.db${suffix}`, { force: true });
}

const server = spawn("./target/debug/server", [], {
  stdio: "inherit",
  env: {
    ...process.env,
    APP__JELLYFIN__ENABLED: "true",
    APP__JELLYFIN__URL: jellyfin.url,
    APP__JELLYFIN__API_KEY: jellyfin.apiKey,
    // Accounts use the LDAP-Auth plugin's login provider (the default), which
    // services.mjs installs and points at the dev LLDAP, as in production.
    APP__SEERR__ENABLED: "true",
    APP__SEERR__URL: seerr.url,
    APP__SEERR__API_KEY: seerr.apiKey,
  },
});

// Make sure stopping the tests also stops the server, so port 3000 is freed.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => server.kill(signal));
}
server.on("exit", (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
