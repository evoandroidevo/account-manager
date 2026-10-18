// Brings the dev Jellyfin and Seerr containers (docker-compose.lldap-dev.yml)
// to a usable state and returns their URLs and API keys. Safe to run any
// number of times: each step checks what already exists.
//
//   node end2end/services.mjs    # prints the result as JSON
//
// The result is also written to data/e2e-services.json so tests can call the
// services' APIs to check what the account manager did.

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Overridable so the setup can be tried against a throwaway container.
export const JELLYFIN_URL = process.env.E2E_JELLYFIN_URL ?? "http://127.0.0.1:8096";
export const SEERR_URL = "http://127.0.0.1:5055";
// Seerr reaches Jellyfin over the compose network.
const JELLYFIN_HOST_FOR_SEERR = "jellyfin";
const ADMIN = { username: "admin", password: "dev-only-password-change-me" };
const KEY_NAME = "account-manager-e2e";
// LDAP-Auth plugin, pointed at the dev LLDAP over the compose network.
const LDAP_PLUGIN = { package: "LDAP Authentication", id: "958aad6637844d2ab89aa7b6fab6e25c" };
const LDAP_SETTINGS = {
  LdapServer: "lldap",
  LdapPort: 3890,
  UseSsl: false,
  UseStartTls: false,
  LdapBindUser: "uid=admin,ou=people,dc=example,dc=com",
  LdapBindPassword: "dev-only-password-change-me",
  LdapBaseDn: "dc=example,dc=com",
  // Members of the "jelly" group may sign in, as in production.
  LdapSearchFilter: "(memberOf=cn=jelly,ou=groups,dc=example,dc=com)",
  LdapAdminBaseDn: "dc=example,dc=com",
  // No such group in the dev LLDAP: nobody becomes a Jellyfin admin through
  // LDAP, so the account manager keeps managing everyone's plan.
  LdapAdminFilter: "(memberOf=cn=jelly_admin,ou=groups,dc=example,dc=com)",
  EnableLdapAdminFilterMemberUid: false,
  LdapUidAttribute: "uid",
  LdapUsernameAttribute: "uid",
  CreateUsersFromLdap: true,
};
const LIBRARIES = [
  { name: "Movies", collectionType: "movies", path: "/media/movies" },
  { name: "Shows", collectionType: "tvshows", path: "/media/shows" },
];

export const SERVICES_FILE = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../data/e2e-services.json",
);

/** Polls `url` until it answers 2xx and `ready(body)` accepts the body. */
async function waitFor(url, what, ready = () => true, timeoutMs = 120_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const response = await fetch(url);
      if (response.ok && ready(await response.clone().text())) return response;
    } catch {
      // not listening yet
    }
    if (Date.now() > deadline) {
      throw new Error(
        `${what} is not reachable at ${url}. Start it with: ` +
          "docker compose -f docker-compose.lldap-dev.yml up -d",
      );
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
}

async function call(method, url, { headers = {}, body } = {}) {
  const response = await fetch(url, {
    method,
    headers: { ...(body ? { "content-type": "application/json" } : {}), ...headers },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) {
    const body = (await response.text()).slice(0, 300);
    throw new Error(`${method} ${url} failed: ${response.status} ${body}`);
  }
  return response;
}

async function json(method, url, options) {
  const response = await call(method, url, options);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

// --- Jellyfin -----------------------------------------------------------------

const CLIENT_HEADER =
  'MediaBrowser Client="account-manager-e2e", Device="e2e", DeviceId="account-manager-e2e", Version="1.0"';

// While starting, Jellyfin serves a "still starting" page with status 200 on
// some routes, so only /health reporting "Healthy" means it's ready.
const jellyfinHealthy = (what) =>
  waitFor(`${JELLYFIN_URL}/health`, what, (body) => body.trim() === "Healthy");

export async function setupJellyfin() {
  await jellyfinHealthy("Jellyfin");
  const info = await json("GET", `${JELLYFIN_URL}/System/Info/Public`);

  if (!info.StartupWizardCompleted) {
    await call("POST", `${JELLYFIN_URL}/Startup/Configuration`, {
      body: { UICulture: "en-US", MetadataCountryCode: "US", PreferredMetadataLanguage: "en" },
    });
    await call("GET", `${JELLYFIN_URL}/Startup/User`);
    await call("POST", `${JELLYFIN_URL}/Startup/User`, {
      body: { Name: ADMIN.username, Password: ADMIN.password },
    });
    await call("POST", `${JELLYFIN_URL}/Startup/RemoteAccess`, {
      body: { EnableRemoteAccess: true },
    });
    await call("POST", `${JELLYFIN_URL}/Startup/Complete`);
  }

  const session = await json("POST", `${JELLYFIN_URL}/Users/AuthenticateByName`, {
    headers: { Authorization: CLIENT_HEADER },
    body: { Username: ADMIN.username, Pw: ADMIN.password },
  });
  const auth = { Authorization: `${CLIENT_HEADER}, Token="${session.AccessToken}"` };

  const folders = await json("GET", `${JELLYFIN_URL}/Library/VirtualFolders`, { headers: auth });
  for (const library of LIBRARIES) {
    if (folders.some((folder) => folder.Name === library.name)) continue;
    const query = new URLSearchParams({
      name: library.name,
      collectionType: library.collectionType,
      refreshLibrary: "false",
    });
    await call("POST", `${JELLYFIN_URL}/Library/VirtualFolders?${query}`, {
      headers: auth,
      body: { LibraryOptions: { PathInfos: [{ Path: library.path }] } },
    });
  }

  const findKey = async () =>
    (await json("GET", `${JELLYFIN_URL}/Auth/Keys`, { headers: auth })).Items.find(
      (key) => key.AppName === KEY_NAME,
    );
  let key = await findKey();
  if (!key) {
    await call("POST", `${JELLYFIN_URL}/Auth/Keys?app=${KEY_NAME}`, { headers: auth });
    key = await findKey();
  }
  await ensureLdapPlugin(auth);
  return { url: JELLYFIN_URL, apiKey: key.AccessToken };
}

/** Installs the LDAP-Auth plugin if needed (which restarts Jellyfin) and
 * points it at the dev LLDAP. Other plugin settings, such as its list of
 * linked users, are kept. */
async function ensureLdapPlugin(auth) {
  const active = async () =>
    (await json("GET", `${JELLYFIN_URL}/Plugins`, { headers: auth })).some(
      (plugin) => plugin.Id === LDAP_PLUGIN.id && plugin.Status === "Active",
    );

  if (!(await active())) {
    // Needs internet access to repo.jellyfin.org. Jellyfin picks the newest
    // version compatible with the server.
    const query = new URLSearchParams({ assemblyGuid: LDAP_PLUGIN.id });
    await call(
      "POST",
      `${JELLYFIN_URL}/Packages/Installed/${encodeURIComponent(LDAP_PLUGIN.package)}?${query}`,
      { headers: auth },
    );
    // The download finishes in the background; the plugin then shows up
    // with status "Restart" until Jellyfin restarts.
    const deadline = Date.now() + 120_000;
    const installed = async () =>
      (await json("GET", `${JELLYFIN_URL}/Plugins`, { headers: auth })).some(
        (plugin) => plugin.Id === LDAP_PLUGIN.id,
      );
    while (!(await installed())) {
      if (Date.now() > deadline) throw new Error("installing the LDAP plugin timed out");
      await new Promise((r) => setTimeout(r, 1000));
    }
    await call("POST", `${JELLYFIN_URL}/System/Restart`, { headers: auth });
    // Wait for the old process to go away, then for the restarted server.
    await new Promise((r) => setTimeout(r, 5000));
    await jellyfinHealthy("Jellyfin (restarting)");
    const ready = Date.now() + 120_000;
    for (;;) {
      try {
        if (await active()) break;
      } catch {
        // still starting up
      }
      if (Date.now() > ready) throw new Error("the LDAP plugin did not become active after restart");
      await new Promise((r) => setTimeout(r, 1000));
    }
  }

  const configUrl = `${JELLYFIN_URL}/Plugins/${LDAP_PLUGIN.id}/Configuration`;
  const current = await json("GET", configUrl, { headers: auth });
  const wanted = { ...current, ...LDAP_SETTINGS };
  if (JSON.stringify(wanted) !== JSON.stringify(current)) {
    await call("POST", configUrl, { headers: auth, body: wanted });
  }
}

// --- Seerr --------------------------------------------------------------------

async function setupSeerr() {
  await waitFor(`${SEERR_URL}/api/v1/status`, "Seerr");
  const settings = await json("GET", `${SEERR_URL}/api/v1/settings/public`);

  // The first Jellyfin administrator to log in becomes Seerr's owner and
  // configures the Jellyfin connection. That setup login's session can't
  // finish initialization, so a normal login (which must not send a host)
  // follows it.
  const MEDIA_SERVER_NOT_CONFIGURED = 4;
  if (settings.mediaServerType === MEDIA_SERVER_NOT_CONFIGURED) {
    await call("POST", `${SEERR_URL}/api/v1/auth/jellyfin`, {
      body: {
        username: ADMIN.username,
        password: ADMIN.password,
        hostname: JELLYFIN_HOST_FOR_SEERR,
        port: 8096,
        useSsl: false,
        urlBase: "",
        email: "admin@example.com",
        serverType: 2, // MediaServerType.JELLYFIN
      },
    });
  }
  // Seerr saves a new login session asynchronously; a request sent right
  // after logging in is occasionally refused with 403 (seen while Seerr is
  // busy with its post-setup Jellyfin scan). Log in again and retry.
  for (let attempt = 1; ; attempt++) {
    const response = await call("POST", `${SEERR_URL}/api/v1/auth/jellyfin`, {
      body: { username: ADMIN.username, password: ADMIN.password },
    });
    const cookie = response.headers
      .getSetCookie()
      .map((value) => value.split(";")[0])
      .join("; ");
    try {
      if (!settings.initialized) {
        await call("POST", `${SEERR_URL}/api/v1/settings/initialize`, { headers: { cookie } });
      }
      const main = await json("GET", `${SEERR_URL}/api/v1/settings/main`, { headers: { cookie } });
      return { url: SEERR_URL, apiKey: main.apiKey };
    } catch (error) {
      if (attempt >= 10 || !String(error).includes(" 403 ")) throw error;
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
}

export async function setupServices() {
  const jellyfin = await setupJellyfin();
  const seerr = await setupSeerr();
  const services = { jellyfin, seerr };
  mkdirSync(dirname(SERVICES_FILE), { recursive: true });
  writeFileSync(SERVICES_FILE, JSON.stringify(services, null, 2));
  return services;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(await setupServices(), null, 2));
}
