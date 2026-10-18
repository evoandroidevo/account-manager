# account-manager

Self-service portal for the homelab: invite-code signup, password/email changes,
paid tiers via Whop, and account provisioning to Jellyfin and Seerr.
LLDAP is the identity source of truth; this app never stores passwords.

## Layout

| Crate      | Role                                                                     |
|------------|--------------------------------------------------------------------------|
| `backend`  | Server-only: config, SQLite repos + migrations, LLDAP client, mailer, account workflows (`accounts.rs`), tiers (`tiers.rs`), Jellyfin/Seerr clients (`services/`), sync worker (`worker.rs`) |
| `app`      | Leptos pages and `#[server]` functions (`ssr` pulls in `backend`)         |
| `frontend` | wasm hydrate entry point                                                 |
| `server`   | axum binary: sessions, CSRF origin guard, `/health`, background tasks    |

## Configuration

Copy `config.example.yaml` to `config.yaml` (or set `APP_CONFIG=/path/to/file.yaml`).
Every key can be overridden as `APP__SECTION__KEY`, e.g. `APP__LDAP__PASSWORD`.

## Tiers and provisioning

Each account is on one tier (`tiers` in the config): an admin override from
`/admin/users`, otherwise `default_tier` (Whop payments come in phase 4). A
background worker applies the tier to every enabled service, in order:

1. **LLDAP groups**: adds the tier's `lldap_groups` and removes groups that
   belong only to other tiers. Groups no tier mentions are never touched.
2. **Jellyfin**: creates missing accounts on the LDAP login provider (so the
   LLDAP password works) and merges the tier into the user policy.
3. **Seerr**: imports the user from Jellyfin and sets quotas and permissions.

Jellyfin administrators and Seerr's owner/admin accounts are left alone.
Existing non-admin accounts **are** updated to their tier's settings on the
first sync, replacing anything set by hand for the fields a tier defines.
Jobs retry with backoff; a reconcile every 10 minutes catches expired plans and
anything missed. Users are queued on signup and on login.

## Pages

| Route | Who | |
|---|---|---|
| `/login` | anyone | LLDAP username and password |
| `/invite?code=…` | anyone with an invite | create an account; logs straight in |
| `/` | user | profile, plan and service status |
| `/security` | user | change password (signs out other devices) |
| `/email` | user | change email via a link sent to the new address |
| `/confirm-email?token=…` | link holder | confirm an email change |
| `/admin/users` | `auth.admin_group` | every user's plan and sync status; set or remove a manual plan, resync |
| `/admin/invites` | `auth.admin_group` | create, list and revoke invite links |

## Development

Needs `cargo-leptos` and the `wasm32-unknown-unknown` target
(Arch: `pacman -S rust-wasm`; rustup: `rustup target add wasm32-unknown-unknown`).

```sh
docker compose -f docker-compose.lldap-dev.yml up -d   # LLDAP (alice / bob), Jellyfin, Seerr
cp config.example.yaml config.yaml                      # then edit for local dev, see below
cargo leptos watch                                      # http://127.0.0.1:3000
```

Local `config.yaml` values for the dev LLDAP:

```yaml
server:   { public_url: http://127.0.0.1:3000, trust_proxy_headers: false }
ldap:     { http_url: http://127.0.0.1:17170, ldap_url: ldap://127.0.0.1:3890,
            base_dn: dc=example,dc=com, username: admin, password: dev-only-password-change-me }
session:  { secure_cookie: false }
mail:     { transport: file, outbox_dir: ./data/outbox }   # emails land here as .eml files
```

Keep `jellyfin.enabled` and `seerr.enabled` false locally unless you mean to
create real accounts for the dev users.

Seed users are listed in [lldap-bootstrap/README.md](lldap-bootstrap/README.md); `bob`
is an admin and can create invite links at `/admin/invites`.
Open the app at exactly the `public_url` origin (`127.0.0.1`, not `localhost`), or
form posts are refused by the CSRF origin check.

## Tests

```sh
cargo test --workspace --exclude frontend      # unit + repo tests

# End-to-end (headless Chromium, hydrated app, real dev LLDAP, Jellyfin and Seerr):
docker compose -f docker-compose.lldap-dev.yml up -d
cargo leptos build
cd end2end && npm ci && npx playwright install chromium   # first time only
npx playwright test
```

The Playwright config starts `target/debug/server` itself on port 3000 (stop any
running dev server first). It uses `end2end/e2e-config.yaml` (never `config.yaml`), its own database
(`data/e2e.db`) and reads emails from `data/e2e-outbox/`. Tests that change a
password or email sign up fresh accounts, so the seed users never change; each run
does add a few `e2e…` users to the dev LLDAP, Jellyfin and Seerr.

Playwright starts the server through `end2end/start-server.mjs`, which first
runs `end2end/services.mjs`: it completes the Jellyfin and Seerr setup wizards
on first use (admin / dev-only-password-change-me, empty Movies and Shows
libraries), installs the LDAP-Auth plugin and points it at the dev LLDAP
(members of `jelly` may sign in), creates API keys, writes them to
`data/e2e-services.json`, and starts the app with both services enabled, as in
production. The first run needs internet access to download the plugin and
takes about a minute. You can run it on its own with `node end2end/services.mjs`. Jellyfin UI: http://127.0.0.1:8096, Seerr UI: http://127.0.0.1:5055.
To start from scratch, remove the containers and their volumes:
`docker compose -f docker-compose.lldap-dev.yml down -v`.
