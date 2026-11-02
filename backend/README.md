# twt-cloud backend

The storage backend of [twt-web](../README.md): reference implementation of the
TWT cloud storage model defined in
[`../docs/DESIGN.md`](../docs/DESIGN.md) and its normative contract
[`../docs/openapi.yaml`](../docs/openapi.yaml).

Rust + Axum + Postgres (sqlx). OIDC bearer auth (JWT via JWKS). The server
stores records (events, tasks, targets, preferences); it never computes worked
time — all computation stays client-side, matching the app's architecture.

```
backend/
├── Dockerfile                 # multi-stage: rust:1-slim build → debian:bookworm-slim, non-root
├── compose.yaml               # base stack: Postgres + API + web + same-origin proxy
├── compose.dev.yaml           # dev overlay: dev JWKS mount + direct API port
├── compose.prod.yaml          # prod overlay: required IdP env + Caddy TLS edge
├── deploy/Caddyfile           # internal same-origin proxy (not the TLS edge)
├── deploy/Dockerfile.caddy    # non-root Caddy image for the proxy/TLS services
├── .env.example               # documented deployment/compose variables
├── migrations/0001_init.sql   # schema v1 (users, clients, change_log, tasks, events, targets, preferences, processed_ops, change_seq)
├── dev/gen-dev-keys.sh        # generates per-developer keypair + JWKS (gitignored outputs)
├── src/
│   ├── lib.rs                 # crate root (lib+bin split for testability)
│   ├── main.rs                # env config, pool, migrate, serve
│   ├── api.rs                 # router: all openapi.yaml paths, nested under /api/v1
│   ├── auth.rs                # OIDC JWT validation (JWKS file or URL)
│   ├── error.rs               # RFC 9457 problem+json
│   └── routes/                # handlers grouped by OpenAPI tag
│       ├── backup.rs          # CSV import/export (byte-compatible with the app)
│       ├── clients.rs         # client registration/revocation
│       ├── config.rs          # /config bootstrap, /oidc-config + /oauth/token browser relays
│       ├── events.rs          # event CRUD
│       ├── identity.rs        # /me, /healthz
│       ├── mod.rs             # Auth extractor + shared helpers
│       ├── preferences.rs     # typed key/value store
│       ├── snapshot.rs        # /export, /import
│       ├── sync.rs            # change-log sync protocol + retention purge
│       ├── targets.rs         # target CRUD
│       └── tasks.rs           # task CRUD
├── tests/                     # integration tests (scratch DB per run)
│   ├── common/mod.rs          # harness: spawn router, forge JWTs, request helpers
│   ├── sync.rs                # cross-tenant, LWW, idempotency, clientId, zoneOffset
│   └── csv_roundtrip.rs       # CSV round-trip byte-stability, clamp on corrupt zone
└── Cargo.toml
```

## Units (faithful to the app)

| wire field      | unit                                      |
|-----------------|-------------------------------------------|
| event `time`    | epoch **seconds**                         |
| `zoneOffset`    | UTC offset in **minutes**                 |
| target `date`   | ISO date (stored as epoch **days**)       |
| target `value`  | target work time in **minutes**           |

Event types: `CLOCK_IN` / `CLOCK_OUT` only (`CLOCK_OUT_NOW` is client-synthesized,
never stored). Target types map to the app's `TargetEnum` readable names
(`day-set`, `day-grant`, `day-ignore`, `flexi-set`, `flexi-add`).

## Running

### Docker Compose (recommended)

Compose is split by environment; the base file has no dev-only settings. CI
publishes multi-arch images (`.github/workflows/docker-publish.yaml`), so a
deployment host needs neither Rust nor Node:

```sh
# deployment: pull the prebuilt images, then run
docker compose -f compose.yaml -f compose.prod.yaml pull
docker compose -f compose.yaml -f compose.prod.yaml up -d

# local development: build from source
docker compose -f compose.yaml -f compose.dev.yaml up -d --build
```

`docker compose pull` fetches
`ghcr.io/<owner>/{twt-cloud,twt-web,twt-proxy}` for amd64 and arm64.
`TWT_IMAGE_PREFIX` (default `ghcr.io/maximilian-staab`) and `TWT_IMAGE_TAG`
(default `latest`) select what it fetches; `--build` tags a local build with
the same names, so `up -d` reuses it. The packages are public; a fork
publishes them under its own owner, where they start private — either make
them public or run `docker login ghcr.io` once on the deployment host.

| file | adds |
| --- | --- |
| `compose.yaml` | Postgres + API + web + same-origin proxy on `127.0.0.1:8080` |
| `compose.dev.yaml` | dev JWKS mount, dev issuer/audience, direct API on `127.0.0.1:9080` |
| `compose.prod.yaml` | required IdP env, no direct API port, Caddy TLS edge on `80`/`443` |

The internal proxy is the same-origin entry: `/` goes to `web`, `/api/v1` to
`api`, so the browser needs no CORS. It is published on
`127.0.0.1:8080`; a TLS-terminating reverse proxy is the public listener and
forwards the public URL there. In dev the API is also published directly on
`127.0.0.1:9080/api/v1` for CLI/plugin clients.

All application containers run as the unprivileged `app` user (UID 10001).
The API image defines a `HEALTHCHECK` against `/api/v1/healthz`, so compose
starts the proxy only once the API is healthy. Base images are pinned by
digest (`rust`, `debian`, `postgres`, `caddy`).

Auth boots with the dev defaults from `compose.dev.yaml`: issuer
`https://dev-idp.example.com`, audience `twt-api`, and the JWKS bind-mounted
from `dev/` (`OAUTH_JWKS_FILE=/app/jwks/jwks.json`). Migrations apply
automatically on startup. The dev overlay sets `I_ACCEPT_DEV_JWKS=1` (dev-only
config).

If the API exits with `migration 1 was previously applied but has been
modified`, the dev volume predates a migration change. Reset it with
`docker compose -f compose.yaml -f compose.dev.yaml down -v` (this destroys
the dev database).

Before the first boot, generate your personal dev keypair:

```sh
./dev/gen-dev-keys.sh
```

The API accepts a token signed by your fresh key. Forge one with any JWT
tooling (e.g. `jwt-cli`) using `dev/dev-key.pem`, `kid dev-key-1`, claims
`sub: user-alice`, `iss: https://dev-idp.example.com`, `aud: twt-api`, and
send it as a Bearer token:

```sh
TOKEN=… # your forged token
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:9080/api/v1/me
```

For a real IdP, copy `.env.example` to `.env` (gitignored) and
edit it. Compose interpolation reads `.env`; `compose.prod.yaml` requires
`POSTGRES_PASSWORD`, `OAUTH_ISSUER_URL`, `OAUTH_AUDIENCE`, `OAUTH_JWKS_URL`
and `TWT_DOMAIN` and refuses to start without them. A complete example:

```sh
cat > .env <<'EOF'
POSTGRES_PASSWORD=change-me
PUBLIC_URL=https://twt.example.com
TWT_DOMAIN=twt.example.com
OAUTH_ISSUER_URL=https://idp.example.com/
OAUTH_AUDIENCE=twt-api
OAUTH_CLIENT_ID=your-client-id
OAUTH_SCOPES=openid,profile,offline_access
OAUTH_JWKS_URL=https://idp.example.com/jwks
EOF
docker compose -f compose.yaml -f compose.prod.yaml up -d --build
```

`OAUTH_JWKS_URL` (live IdP endpoint) wins over `OAUTH_JWKS_FILE` inside the
container — see `src/auth.rs` — so pointing `.env` at a real IdP disables
the dev-JWKS path without touching the compose file.

### Local dev (cargo)

```sh
# Postgres
docker run -d --name twt-pg -e POSTGRES_USER=twt -e POSTGRES_PASSWORD=twt-dev \
  -e POSTGRES_DB=twt_cloud -p 5432:5432 postgres:16-alpine

DATABASE_URL=postgres://twt:twt-dev@localhost:5432/twt_cloud \
BIND_ADDR=127.0.0.1:8080 \
OAUTH_ISSUER_URL=https://dev-idp.example.com \
OAUTH_AUDIENCE=twt-api \
OAUTH_CLIENT_ID=twt-app \
OAUTH_JWKS_FILE=$PWD/dev/jwks.json \
I_ACCEPT_DEV_JWKS=1 \
cargo run
```

`I_ACCEPT_DEV_JWKS=1` is required whenever `OAUTH_JWKS_FILE` is set: without
it the API refuses to boot rather than silently trust a static key file.

### Shutdown

SIGTERM (and Ctrl-C) trigger a graceful shutdown: the listener stops
accepting, in-flight requests drain to completion, the retention-purge task
is cancelled and the pool is closed before exit 0. In Docker this means
`docker stop` returns as soon as the drain finishes instead of escalating to
SIGKILL after the 10 s grace period.

### Environment variables

| var                  | meaning                                             |
|----------------------|-----------------------------------------------------|
| `DATABASE_URL`       | Postgres DSN                                        |
| `BIND_ADDR`          | listen address (default `0.0.0.0:8080`)             |
| `OAUTH_ISSUER_URL`   | expected `iss` claim                                |
| `OAUTH_AUDIENCE`     | expected `aud` claim                                |
| `OAUTH_JWKS_FILE`    | **dev**: static JWKS file (no IdP needed)           |
| `OAUTH_JWKS_URL`     | **prod**: JWKS endpoint of the IdP                  |
| `I_ACCEPT_DEV_JWKS`  | **dev**: must be `1` to boot with `OAUTH_JWKS_FILE` |
| `OAUTH_CLIENT_ID`    | public OAuth client id advertised via `GET /api/v1/config` (default empty) |
| `OAUTH_SCOPES`       | comma-separated scopes advertised via `GET /api/v1/config` (default `openid,profile,offline_access`) |
| `POSTGRES_PASSWORD`  | **compose**: Postgres password (dev default `twt-dev`; required by `compose.prod.yaml`) |
| `TWT_DOMAIN`         | **compose.prod.yaml**: bare public domain, e.g. `twt.example.com` |
| `PUBLIC_URL`         | public origin of the web app (`scheme://host[:port]`, no path); compose uses it as the default `CORS_ALLOWED_ORIGINS` for a separate frontend origin |
| `CORS_ALLOWED_ORIGINS` | comma-separated exact origins allowed to call the API from a browser (`*` for any); unset = no CORS headers. The compose proxy is same-origin, so this is empty by default; set it (or `PUBLIC_URL`) only when the frontend has its own origin |
| `RETENTION_DAYS`    | change-log retention window in days (default 180)   |

### Dev JWT

The dev keypair is **per developer** — generated locally, never committed:

```sh
./dev/gen-dev-keys.sh          # creates dev/dev-key.pem + dev/jwks.json
```

`dev-key.pem` (RSA-2048) signs tokens; `jwks.json` is the matching JWKS
(`kid: dev-key-1`). Forge a token with any JWT tooling using `dev/dev-key.pem`,
claims `sub: user-alice`, `aud: twt-api`, `iss: https://dev-idp.example.com`.
When `OAUTH_JWKS_FILE` is set, the API refuses to boot unless
`I_ACCEPT_DEV_JWKS=1` is set: a static JWKS trusts a key whose private half
may not be yours, so the risk must be accepted explicitly per boot.

The old shared key (committed until now) is dead: it has been purged from git
history, and every deployment that used it must regenerate their own keypair
via the script.

## Smoke test

```sh
./dev/gen-dev-keys.sh                # if not generated yet
TOKEN=$(cat dev/token.txt)           # signed by your fresh key
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/api/v1/me
```

Note: all routes are nested under `/api/v1` per the OpenAPI contract
(`servers:` block). The token must match the compose env
(`iss: https://dev-idp.example.com`, `aud: twt-api`).

## CSV backup byte-compatibility

`GET/PUT /backup/{events,targets,preferences}` produce/consume the exact CSV
formats of the app:

* events: `type;time;task;comment;taskId;name;active;ordering;isdefault`,
  `in`/`out` localized type names, `ISO_OFFSET_DATE_TIME` timestamps,
  per-user integer task numbering (`tasks.local_id`), one row per task
  without events
* targets: `time;type;value;comment`, ISO dates, readable type names
* preferences: `key;type;value` lines (no header), types `string`,
  `stringset`, `boolean`, `integer`, `float`, `long`

Verified round-trips (live smoke run, 2026-10-19): `export → import → export`
is byte-stable for all three CSVs. Verified behaviors on the same run:
invalid enum types → 400 problem+json (not 500); PATCH with a stale
`updatedAt` → 412; `/sync/push` ops older than the stored row →
`ignoredStale`; `/sync/push` bodies over 1 MiB → 413; 401s carry
`WWW-Authenticate: Bearer error="invalid_token"`.

## Tests

Integration tests (`tests/`) run the real router against a scratch Postgres
database (unique per test run; created and dropped by the harness). They forge
RS256 JWTs with `dev/dev-key.pem` and validate them against `dev/jwks.json`,
exercising the full auth path in-process.

Coverage:

* cross-tenant `/sync/push` writes are `rejected` and never touch another
  user's rows
* `clientId` ownership: foreign/unknown → 403, missing → 422, revoked → 403
* LWW: fresh → `applied`, stale → `ignoredStale`, same opId → `duplicate`
* `zoneOffset` bounds (±1080 min): REST → 400 problem+json, sync → op rejected
* `/backup/events` CSV round-trip byte-stability; corrupt `zone_offset`
  (beyond ±18h) clamps on export instead of panicking
* `/events` list pagination: `limit` clamps to 1..=1000 (default 500) and
  `offset` to >= 0, matching `/sync/changes`; default/max/last/from pages
  stay exact
* `/targets` list pagination: same `limit`/`offset` clamping and ascending
  `(date, id)` order, so clients can page past the old 500-row cap

* retention purge: expired change-log rows are deleted and the per-user
  watermark (`change_retention.purged_through_seq`) recorded; a cursor inside
  the purged range gets `410` problem+json (`type: cursor-expired`),
  `cursor=0` and cursors at the watermark still page normally

Run locally (default: `postgres://twt:twt-dev@127.0.0.1:5432/postgres`,
matching the devenv `backend` profile's Postgres that `devenv up` starts;
override with `TEST_DATABASE_URL`):

```sh
cargo test
```


## Production deployment

The binary is intentionally thin: rate limiting, TLS termination and metrics
are delegated to the infrastructure in front of it. A minimal production
setup:

1. **Reverse proxy** (nginx, Traefik, Caddy, …) terminating TLS. Forward the
   public URL to the compose proxy on `127.0.0.1:8080`; it routes `/` and
   `/api/v1` on one origin. The TLS terminator is the only public listener and
   owns the HTTP→HTTPS redirect and HSTS. If the terminator runs as a
   container, attach it to the compose network and target `proxy:80` instead
   of a host port.
2. **Rate limits** (recommended, proxy-level): 10 req/s sustained per token,
   100 req/s burst (see `DESIGN.md` §11). Without them, the unauthenticated
   surface (`/healthz`) and the authenticated endpoints are unbounded.
3. **Postgres 16** with automated backups; `RETENTION_DAYS` sized to your
   change-log growth (default 180).
4. **Secrets**: real IdP values in the environment, never
   `OAUTH_JWKS_FILE`/`I_ACCEPT_DEV_JWKS` (dev-only paths). `OAUTH_JWKS_URL`
   wins over the dev file, so the compose dev mount/flag are inert once it is
   set; use `compose.prod.yaml`, which omits them, for a strict production
   setup.

### Real domain

1. Point `twt.example.com` (A/AAAA) at the host and allow inbound `80`/`443`.
   The compose proxy (`127.0.0.1:8080`) and direct API (`127.0.0.1:9080`)
   stay off the public interface.
2. Choose the TLS edge:
   - **Existing terminator**: forward `https://twt.example.com` to
     `http://127.0.0.1:8080` and set HSTS there.
   - **Built-in TLS edge** (`compose.prod.yaml`): set
     `TWT_DOMAIN=twt.example.com` in `.env`, then `docker compose -f
     compose.yaml -f compose.prod.yaml pull` and `up -d` (add `--build` only
     when deploying from source).
     Caddy obtains/renews the certificate, redirects HTTP→HTTPS, and adds
     HSTS. Do not enable it if another proxy already owns `80`/`443`.
     Certificate volumes created by an older root-running stack are
     root-owned: recreate `caddy_data`/`caddy_config` (or chown them to
     `10001:10001`) before switching to the non-root image.
3. Set `POSTGRES_PASSWORD` and the real IdP values in `.env`.
   Existing volumes keep the password they were initialised with; use a
   fresh volume for a fresh deployment. Use a URL-safe password (letters,
   digits, `-._~`): it is embedded in `DATABASE_URL`, so reserved characters
   would need percent-encoding.
4. Register `https://twt.example.com/` as the OAuth redirect URI (the page
   strips `index.html`). Remote app/plugin clients use
   `https://twt.example.com/api/v1`; the direct `:9080` API is local-only.
5. In the web login form, enter `https://twt.example.com` (or
   `https://twt.example.com/api/v1`); the built-in default is the local
   compose URL.
6. Verify:

```sh
curl -s https://twt.example.com/api/v1/healthz
curl -sI https://twt.example.com/ | grep -iE 'strict-transport|content-security|x-frame'
```

A host-level Caddy equivalent to the built-in `tls` profile:

```caddy
twt.example.com {
	header Strict-Transport-Security "max-age=31536000; includeSubDomains"
	reverse_proxy 127.0.0.1:8080
}
```

`compose.prod.yaml` is the single-node reference: it pulls the prebuilt API,
web, proxy and TLS images (`docker compose pull`) and falls back to a local
build only when `--build` is passed. Pin a release instead of tracking
`latest`, so a redeploy is a deliberate step:

```sh
# .env: TWT_IMAGE_TAG=v1.2.3
docker compose -f compose.yaml -f compose.prod.yaml pull
docker compose -f compose.yaml -f compose.prod.yaml up -d
```

The same images work without compose. Each service only needs its environment
plus the config/compose wiring:

| image | needs |
| --- | --- |
| `ghcr.io/<owner>/twt-cloud:<tag>` | `DATABASE_URL`, the OAuth vars; Postgres reachable; listens on `BIND_ADDR` (default `0.0.0.0:8080`), health at `/api/v1/healthz` |
| `ghcr.io/<owner>/twt-web:<tag>` | optionally `TWT_CSP_CONNECT_SRC`; serves `/` on `:80`, health at `/healthz` |
| `ghcr.io/<owner>/twt-proxy:<tag>` | a Caddyfile mounted at `/etc/caddy/Caddyfile` (the repo ships `deploy/Caddyfile` for the same-origin proxy and `deploy/domain.Caddyfile` for the TLS edge) and, for the edge, the `caddy_data`/`caddy_config` volumes |

The proxy image contains no configuration on purpose: the same image runs as
the internal proxy (`api:8080` + `web:80`) and as the TLS edge (`proxy:80`).

Requirements not covered by the compose file (backups, monitoring, log
shipping) stay operator responsibility.

## CI

GitHub Actions runs on every push and pull request. Backend jobs live in
[`../.github/workflows/backend-tests.yaml`](../.github/workflows/backend-tests.yaml):

| job | command |
| --- | --- |
| `fmt` | `cargo fmt --all --check` |
| `clippy` | `cargo clippy --all-targets --locked -- -D warnings` |
| `test` | `cargo test --all-targets --locked` (against a `postgres:16-alpine` service) |
| `msrv` | `cargo check --all-targets --locked` on the `rust-version` from `Cargo.toml` (1.95) |
| `deny` | `cargo deny check` — licenses, RustSec advisories, yanked crates, wildcards, sources (`deny.toml`) |

`Cargo.lock` is committed, so every cargo invocation uses `--locked`.

Other workflows: [`docker.yaml`](../.github/workflows/docker.yaml) builds all
three images and boots `compose.yaml` + `compose.dev.yaml` until every
healthcheck passes, then probes the proxy; `docker-publish.yaml` pushes the
multi-arch `ghcr.io/<owner>/{twt-cloud,twt-web,twt-proxy}` images on `main` and
`v*` tags; `secret-scan.yaml` runs gitleaks over the full history;
`openapi-lint.yaml` lints [`../docs/openapi.yaml`](../docs/openapi.yaml) with
the rules in [`../redocly.yaml`](../redocly.yaml).

## Security invariants (verified live)

* Upserts are user-scoped: `ON CONFLICT … WHERE <table>.user_id = $2` — a
  forged token cannot overwrite another user's event/task/target rows; the op
  comes back `rejected` with reason `id belongs to another user`.
* `/sync/push` requires a `clientId` owned by the caller and not revoked;
  otherwise 403 problem+json. This user-scopes `(client_id, op_id)` in
  `processed_ops` transitively.
* `zoneOffset` is bounded to ±1080 minutes (Java `ZoneOffset`) on every input
  path (REST create/update, sync push, snapshot import); the CSV export clamps
  legacy out-of-range rows instead of panicking.
* Every mutation (entity write + change-log append + processed-ops ledger)
  runs in a single transaction — a crash can no longer leave a written row
  without its change-log entry, which would break delta sync.

## License

GPL-3.0-or-later — see [LICENSE](LICENSE). Declared in `Cargo.toml`
(`license = "GPL-3.0-or-later"`).
