// Removes the accounts the end-to-end tests create (usernames from
// uniqueUser() in tests/helpers.ts: "e2e" + base-36 digits) from the dev
// LLDAP, Jellyfin and Seerr. The seeded users (alice, bob, admin) and the
// service admins are never touched.
//
// Runs as Playwright's global teardown, and by hand:
//
//   node end2end/cleanup.mjs

import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { SERVICES_FILE } from "./services.mjs";

const E2E_USERNAME = /^e2e[0-9a-z]+$/;
const E2E_EMAIL = /^e2e[0-9a-z]+@example\.com$/;
const LLDAP_URL = "http://127.0.0.1:17170";
const LLDAP_ADMIN = { username: "admin", password: "dev-only-password-change-me" };

async function request(method, url, headers, body) {
  const response = await fetch(url, {
    method,
    headers: { ...(body ? { "content-type": "application/json" } : {}), ...headers },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) {
    throw new Error(`${method} ${url} failed: ${response.status} ${(await response.text()).slice(0, 200)}`);
  }
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

// LLDAP goes first: without the LLDAP account the account manager can't
// re-create the Jellyfin and Seerr ones from a sync job still in the queue.
async function cleanLldap() {
  const { token } = await request("POST", `${LLDAP_URL}/auth/simple/login`, {}, LLDAP_ADMIN);
  const graphql = (query, variables) =>
    request("POST", `${LLDAP_URL}/api/graphql`, { authorization: `Bearer ${token}` }, { query, variables });
  const { data } = await graphql("{ users { id } }");
  const ids = data.users.map((user) => user.id).filter((id) => E2E_USERNAME.test(id));
  for (const id of ids) {
    await graphql("mutation ($id: String!) { deleteUser(userId: $id) { ok } }", { id });
  }
  return ids.length;
}

// Seerr before Jellyfin, since its users were imported from Jellyfin.
async function cleanSeerr({ url, apiKey }) {
  const headers = { "X-Api-Key": apiKey };
  const page = await request("GET", `${url}/api/v1/user?take=1000`, headers);
  const users = page.results.filter(
    (user) =>
      user.id !== 1 &&
      (E2E_USERNAME.test(user.jellyfinUsername ?? "") || E2E_EMAIL.test(user.email ?? "")),
  );
  for (const user of users) {
    await request("DELETE", `${url}/api/v1/user/${user.id}`, headers);
  }
  return users.length;
}

async function cleanJellyfin({ url, apiKey }) {
  const headers = { authorization: `MediaBrowser Token="${apiKey}"` };
  const users = (await request("GET", `${url}/Users`, headers)).filter(
    (user) => E2E_USERNAME.test(user.Name) && !user.Policy?.IsAdministrator,
  );
  for (const user of users) {
    await request("DELETE", `${url}/Users/${user.Id}`, headers);
  }
  return users.length;
}

export default async function cleanup() {
  const removed = { lldap: await cleanLldap() };
  // Written by services.mjs when the server starts; absent if it never did.
  if (existsSync(SERVICES_FILE)) {
    const { jellyfin, seerr } = JSON.parse(readFileSync(SERVICES_FILE, "utf8"));
    removed.seerr = await cleanSeerr(seerr);
    removed.jellyfin = await cleanJellyfin(jellyfin);
  }
  console.log(
    `removed e2e accounts: ${Object.entries(removed)
      .map(([service, count]) => `${count} from ${service}`)
      .join(", ")}`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await cleanup();
}
