# LLDAP bootstrap data

Seed data for `docker-compose.lldap-dev.yml`, which runs `lldap/lldap:stable` and then
`/app/bootstrap.sh` against it. Development only.

Groups mirror the homelab: `user`, `jelly`, `admins`.

| User  | Password         | Groups          |
|-------|------------------|-----------------|
| alice | `AliceDemo-123!` | user, jelly     |
| bob   | `BobDemo-123!`   | user, admins    |
