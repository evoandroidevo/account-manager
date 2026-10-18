# account-manager

Self-service portal for the homelab: invite-code signup, password/email changes,
paid tiers via Whop, and account provisioning to Jellyfin and Seerr.
LLDAP is the identity source of truth; this app never stores passwords.

## Layout

| Crate      | Role                                                                     |
|------------|--------------------------------------------------------------------------|
| `backend`  | Server-only: config, SQLite repos + migrations, LLDAP client, mailer, account workflows (`accounts.rs`) |
| `app`      | Leptos pages and `#[server]` functions (`ssr` pulls in `backend`)         |
| `frontend` | wasm hydrate entry point                                                 |
| `server`   | axum binary: sessions, CSRF origin guard, `/health`, background tasks    |

## Configuration

Copy `config.example.yaml` to `config.yaml` (or set `APP_CONFIG=/path/to/file.yaml`).
Every key can be overridden as `APP__SECTION__KEY`, e.g. `APP__LDAP__PASSWORD`.

## Pages

| Route | Who | |
|---|---|---|
| `/login` | anyone | LLDAP username and password |
| `/invite?code=…` | anyone with an invite | create an account; logs straight in |
| `/` | user | profile and groups |
| `/security` | user | change password (signs out other devices) |
| `/email` | user | change email via a link sent to the new address |
| `/confirm-email?token=…` | link holder | confirm an email change |
| `/admin/invites` | `auth.admin_group` | create, list and revoke invite links |

## Development

Needs `cargo-leptos` and the `wasm32-unknown-unknown` target
(Arch: `pacman -S rust-wasm`; rustup: `rustup target add wasm32-unknown-unknown`).

```sh
docker compose -f docker-compose.lldap-dev.yml up -d   # LLDAP seeded with alice / bob
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

Seed users are listed in [lldap-bootstrap/README.md](lldap-bootstrap/README.md); `bob`
is an admin and can create invite links at `/admin/invites`.
Open the app at exactly the `public_url` origin (`127.0.0.1`, not `localhost`), or
form posts are refused by the CSRF origin check.

## Tests

```sh
cargo test --workspace --exclude frontend      # unit + repo tests

# End-to-end (headless Chromium, hydrated app, real dev LLDAP):
docker compose -f docker-compose.lldap-dev.yml up -d
cargo leptos build
cd end2end && npm ci && npx playwright install chromium   # first time only
npx playwright test
```

The Playwright config starts `target/debug/server` itself on port 3000 (stop any
running dev server first). It ignores `config.yaml`, uses its own database
(`data/e2e.db`) and reads emails from `data/e2e-outbox/`. Tests that change a
password or email sign up fresh accounts, so the seed users never change; each run
does add a few `e2e…` users to the dev LLDAP.
