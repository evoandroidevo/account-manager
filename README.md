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

### Environment variables

Environment variables override `config.yaml`, which overrides the defaults below.
Names are `APP__` plus the YAML path in upper case, joined with `__`
(`mail.smtp.password` → `APP__MAIL__SMTP__PASSWORD`). Lists (`tiers`,
`invites.default_groups`) can only be set in the YAML file. Secrets are marked 🔑:
prefer setting these as variables over writing them into the file.

| Variable | Default | Description |
|---|---|---|
| `APP_CONFIG` | `config.yaml` (`/app/config.yaml` in the image) | Path of the YAML config file; a missing file means defaults plus variables |
| **Server** | | |
| `APP__SERVER__PUBLIC_URL` | `http://127.0.0.1:3000` | Public origin as the browser sees it; POSTs from other origins are refused, and email links use it |
| `APP__SERVER__TRUST_PROXY_HEADERS` | `false` | Take the client IP from `X-Forwarded-For` (only behind Caddy) |
| `APP__DATABASE__URL` | `sqlite://./data/account-manager.db` | SQLite database |
| `APP__DEFAULT_TIER` | `free` | Tier for accounts without a paid or admin-set plan |
| **LLDAP** | | |
| `APP__LDAP__HTTP_URL` | `http://127.0.0.1:17170` | LLDAP web/GraphQL address |
| `APP__LDAP__LDAP_URL` | `ldap://127.0.0.1:3890` | LDAP address, for checking passwords |
| `APP__LDAP__USE_TLS` | `false` | Connect with LDAPS (`ldap://` becomes `ldaps://`) |
| `APP__LDAP__TLS_INSECURE_SKIP_VERIFY` | `false` | Skip certificate checks (testing only) |
| `APP__LDAP__TLS_CA_FILE` | none | Extra CA certificate (PEM) for LDAP TLS |
| `APP__LDAP__BASE_DN` | `dc=example,dc=com` | LLDAP base DN |
| `APP__LDAP__USERNAME` | `admin` | Service account (in `lldap_admin` or `lldap_password_manager`) |
| `APP__LDAP__PASSWORD` 🔑 | `dev-only-password-change-me` | Its password |
| **Invites, sessions and login** | | |
| `APP__INVITES__EXPIRATION_HOURS` | `48` | How long an invite link works |
| `APP__SESSION__COOKIE_NAME` | `am_session` | Session cookie name |
| `APP__SESSION__SECURE_COOKIE` | `true` | HTTPS-only cookie; `false` only for local plain HTTP |
| `APP__SESSION__IDLE_TIMEOUT_MINUTES` | `60` | Sign out after this long without activity |
| `APP__SESSION__ABSOLUTE_TIMEOUT_HOURS` | `12` | Sign out this long after logging in, regardless |
| `APP__AUTH__ADMIN_GROUP` | `admins` | LLDAP group that gets the admin pages |
| `APP__AUTH__LOGIN_MAX_ATTEMPTS` | `5` | Failed logins allowed per username and IP within the window |
| `APP__AUTH__LOGIN_WINDOW_MINUTES` | `15` | Window for the limit above |
| **Password policy** | | |
| `APP__PASSWORD_POLICY__MINIMUM_LENGTH` | `12` | Minimum password length |
| `APP__PASSWORD_POLICY__REQUIRE_UPPERCASE` | `true` | Needs an uppercase letter |
| `APP__PASSWORD_POLICY__REQUIRE_LOWERCASE` | `true` | Needs a lowercase letter |
| `APP__PASSWORD_POLICY__REQUIRE_NUMBER` | `true` | Needs a digit |
| `APP__PASSWORD_POLICY__REQUIRE_SPECIAL` | `true` | Needs a symbol |
| **Mail** | | |
| `APP__MAIL__TRANSPORT` | `log` | `smtp`, `file` (writes .eml files to `outbox_dir`) or `log` (only logs) |
| `APP__MAIL__FROM` | `Account Manager <no-reply@example.com>` | Sender address |
| `APP__MAIL__SMTP__HOST` | `localhost` | SMTP server |
| `APP__MAIL__SMTP__PORT` | `587` | SMTP port |
| `APP__MAIL__SMTP__STARTTLS` | `true` | STARTTLS; `false` uses implicit TLS (port 465) |
| `APP__MAIL__SMTP__USERNAME` | empty | SMTP username |
| `APP__MAIL__SMTP__PASSWORD` 🔑 | empty | SMTP password |
| `APP__MAIL__OUTBOX_DIR` | `./data/outbox` | Folder for the `file` transport |
| `APP__MAIL__EMAIL_CHANGE_LINK_HOURS` | `24` | How long an email-change confirmation link works |
| **Jellyfin** | | |
| `APP__JELLYFIN__ENABLED` | `false` | Create and update Jellyfin accounts |
| `APP__JELLYFIN__URL` | empty | Jellyfin address, including any base URL (`http://jellyfin:8096/jellyfin`) |
| `APP__JELLYFIN__API_KEY` 🔑 | empty | Dashboard → API Keys |
| `APP__JELLYFIN__AUTH_PROVIDER_ID` | `Jellyfin.Plugin.LDAP_Auth.LdapAuthenticationProviderPlugin` | Login provider for new accounts (the LDAP-Auth plugin) |
| `APP__JELLYFIN__PASSWORD_RESET_PROVIDER_ID` | `Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider` | Password reset provider for new accounts |
| `APP__JELLYFIN__LOGIN_ATTEMPTS_BEFORE_LOCKOUT` | unset (leave accounts alone) | Failed logins before Jellyfin locks an account: `-1` never, `0` Jellyfin's default (3) |
| **Seerr** | | |
| `APP__SEERR__ENABLED` | `false` | Create and update Seerr accounts (needs Jellyfin) |
| `APP__SEERR__URL` | empty | Seerr address |
| `APP__SEERR__API_KEY` 🔑 | empty | Settings → General → API Key |
| **Whop** | | |
| `APP__WHOP__ENABLED` | `false` | Paid plans through Whop |
| `APP__WHOP__API_URL` | `https://api.whop.com/api/v1` | API address (sandbox: `https://sandbox-api.whop.com/api/v1`) |
| `APP__WHOP__API_KEY` 🔑 | empty | Company API key (`apik_…`) |
| `APP__WHOP__ACCOUNT_ID` | empty | Company id (`biz_…`) |
| `APP__WHOP__WEBHOOK_SECRET` 🔑 | empty | Webhook signing secret (`ws_…`) |
| `APP__WHOP__GRACE_DAYS` | `3` | Paid access lasts this many days past the end of the billing period |

The image also sets `LEPTOS_SITE_ADDR=0.0.0.0:3000` (listen address),
`LEPTOS_SITE_ROOT`, `LEPTOS_OUTPUT_NAME`, `LEPTOS_SITE_PKG_DIR` and
`LEPTOS_ENV=PROD`; only `LEPTOS_SITE_ADDR` is worth changing.

## Tiers and provisioning

Each account is on one tier (`tiers` in the config): an admin override from
`/admin/users`, else a paid Whop membership, else `default_tier`. A
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

## Payments (Whop)

`/billing` lists the tiers; a tier with `whop_plan_ids` gets a buy button that
creates a Whop checkout carrying the user's id as metadata and sends the
browser there. Whop then calls `POST /api/webhooks/whop`:

- The signature is checked against the raw body (Standard Webhooks headers,
  HMAC-SHA256 keyed with the literal `ws_…` secret, 5-minute window), and each
  delivery id is processed once.
- `membership.*` events are matched to an account by the checkout metadata,
  then a known membership, then the account with the same email. The user's
  best active membership sets their paid tier until the billing period ends
  plus `grace_days`; cancelled or expired memberships remove it.
- Memberships that match nobody (bought on Whop directly with another email)
  wait on `/admin/payments` until an admin assigns them.

Changing plans (`backend/src/plan_change.rs`); Whop can't move a membership
to another plan, so:

- **Upgrades** are a new checkout whose first payment is reduced by the unused
  part of the current period (old renewal price × time left, capped at the new
  first payment, skipped below $0.50 or while trialing). The credit is a
  single-use promo code limited to the new plan, applied with `?promoCode=` on
  the checkout link. Once the new membership is active the old one is
  cancelled immediately. The API key needs the promo code and membership
  management permissions.
- **Downgrades** (including to the free tier) are scheduled: the current
  membership stops renewing and runs to the end of its period, and the page
  offers "Keep <plan>" until then. After it ends, a paid target is offered as
  "Continue with <plan>".

A reconcile at startup and daily re-applies every membership Whop reports,
in case a webhook was missed. In production the webhook path must bypass
Authelia; the signature is its authentication.

## Pages

| Route | Who | |
|---|---|---|
| `/login` | anyone | LLDAP username and password |
| `/invite?code=…` | anyone with an invite | create an account; logs straight in |
| `/` | user | profile, plan and service status |
| `/billing` | user | compare plans, buy one through Whop, manage the subscription |
| `/security` | user | change password (signs out other devices) |
| `/email` | user | change email via a link sent to the new address |
| `/confirm-email?token=…` | link holder | confirm an email change |
| `/admin/users` | `auth.admin_group` | every user's plan and sync status; set or remove a manual plan, resync |
| `/admin/invites` | `auth.admin_group` | create, list and revoke invite links |
| `/admin/payments` | `auth.admin_group` | unmatched Whop memberships (assign to an account) and recent webhooks |

## Deployment

Images are published to `ghcr.io/evoandroidevo/account-manager` by
`.github/workflows/docker.yml`, after the backend tests pass:

| Trigger | Tags |
|---|---|
| push to `main` | `edge`, `sha-<commit>` |
| tag `v1.2.3` | `1.2.3`, `1.2`, `latest`, `sha-<commit>` |

To release, bump `version` in `server/Cargo.toml`, then `git tag v1.2.3 && git push origin v1.2.3`.
The tag also runs `.github/workflows/release.yml`, which creates the GitHub
release with standalone archives (see below).

The image (`Dockerfile`, linux/amd64 and linux/arm64, e.g. a Raspberry Pi 4/5 on a 64-bit OS) runs as uid 10001, listens on port 3000,
reads `/app/config.yaml` and keeps the SQLite database in `/app/data`.
`compose.example.yaml` shows a deployment; secrets can stay out of the config
file as `APP__SECTION__KEY` variables. Its `HEALTHCHECK` runs
`account-manager healthcheck`, which asks the server's `/health`.

Behind Caddy, set `server.public_url` to the public origin and
`server.trust_proxy_headers: true`. Don't put the site behind Authelia's
forward auth (it does its own LLDAP login), and keep `/api/webhooks/whop`
reachable by Whop (its signature is the authentication; no bot challenge).

The GHCR package starts private: make it public in the package settings on
GitHub, or `docker login ghcr.io` on the server with a token that has
`read:packages`.

### Standalone (without Docker)

Each GitHub release has `account-manager-<version>-x86_64-unknown-linux-gnu.tar.gz`
(glibc 2.35 or newer) and `account-manager-<version>-x86_64-pc-windows-msvc.zip`
(64-bit Windows 10/11 or Server 2016+, no Visual C++ runtime needed),
plus `SHA256SUMS`. Each holds the `account-manager` executable, its `site/`
folder (keep them together), `config.example.yaml`, this README and the licence.
Running the workflow by hand (Actions → Release → Run workflow) builds the
same archives as workflow artifacts without making a release.

Extract it, copy `config.example.yaml` to `config.yaml` beside the executable,
and start it from that folder (the config file and `data/` are looked up in the
current directory):

```sh
./account-manager                                # Linux
LEPTOS_SITE_ADDR=0.0.0.0:3000 ./account-manager  # listen on every interface
```

On Windows run `account-manager.exe` the same way (`$env:LEPTOS_SITE_ADDR="0.0.0.0:3000"`
in PowerShell). It listens on `127.0.0.1:3000` unless `LEPTOS_SITE_ADDR` says
otherwise; all other settings are the environment variables above.

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
(`data/e2e.db`, emptied at the start of each run) and reads emails from
`data/e2e-outbox/`. Tests that change a password or email sign up fresh accounts,
so the seed users never change. Those `e2e…` accounts are deleted from the dev
LLDAP, Jellyfin and Seerr when the run ends (`end2end/cleanup.mjs`, also runnable
on its own with `node end2end/cleanup.mjs` after an interrupted run).

Payments run against `end2end/mock-whop.mjs`, a stand-in for Whop's API and
checkout on port 3011 that signs webhooks the way Whop does, so the whole buy,
upgrade, downgrade, cancel and unmatched-payment flow is tested without a Whop account.

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
