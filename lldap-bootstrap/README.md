# LLDAP bootstrap data

Seed data for `docker-compose.lldap-dev.yml`, which runs `lldap/lldap:stable` and then
`/app/bootstrap.sh` against it. Development only.

Groups mirror the homelab: `user`, `jelly`, `admins`, plus `plus`, `premium` and `premiumplus` for the paid tiers.
After adding groups or users here, re-run the seeding with
`docker compose -f docker-compose.lldap-dev.yml up -d --force-recreate lldap-bootstrap`.

| User  | Password         | Groups          |
|-------|------------------|-----------------|
| alice | `AliceDemo-123!` | user, jelly     |
| bob   | `BobDemo-123!`   | user, admins    |
