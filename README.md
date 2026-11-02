<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# twt-web

Self-hosted storage backend and browser frontend for
[TrackWorkTime](https://codeberg.org/mathisdt/trackworktime) time tracking.

The backend (`twt-cloud`) stores events, tasks, targets and preferences over a
REST API with a change-log sync protocol; the web client is a build-free UI on
top of it. All work-time math stays client-side — the server never computes
worked time.

## Layout

```
├── docs/            # backend design + normative OpenAPI contract
├── backend/         # twt-cloud: Rust (Axum + sqlx) + Postgres storage backend
├── web/             # build-free browser frontend (OAuth PKCE + REST)
└── .github/         # CI workflows and dependabot
```

## What's here

- **`docs/`** — `DESIGN.md` (storage-only backend spec: multi-client sync,
  OIDC auth, change-log protocol) and `openapi.yaml` (OpenAPI 3.1, the
  normative REST contract).
- **`backend/`** — reference implementation of the contract: events, tasks,
  targets, preferences CRUD; change-log sync; CSV backup byte-compatible with
  the Android app. Ships `Dockerfile` + `compose.yaml` with `compose.dev.yaml`
  and `compose.prod.yaml` overlays, a same-origin proxy on `:8080`, and the
  direct API on `:9080/api/v1` in dev. See [`backend/README.md`](backend/README.md).
- **`web/`** — build-free browser frontend: OAuth 2.0 Authorization Code +
  PKCE sign-in (same `/config` → discovery → refresh flow as the app), dashboard
  with worked-today/flexi/week table and clock in/out, plus events, tasks and
  targets CRUD. Ships a `Dockerfile` (Caddy) and runs as the `web` service in
  the backend compose stack. See [`web/README.md`](web/README.md).

## Related repositories

- Android app with cloud sync support —
  [twt-cloud on Codeberg](https://git.faxen.eu/qdulf/twt-cloud).
- DankMaterialShell plugin —
  [Maximilian-Staab/twt-dms-plugin](https://github.com/Maximilian-Staab/twt-dms-plugin).

## Quick start (backend)

```sh
cd backend
docker compose -f compose.yaml -f compose.dev.yaml up -d --build
curl -s http://localhost:8080/api/v1/healthz   # {"status":"ok"}
```

A deployment does not build anything: CI publishes multi-arch images
(`ghcr.io/<owner>/{twt-cloud,twt-web,twt-proxy}`, amd64 + arm64) and compose
pulls them. Copy `.env.example` to `.env`, set the required values, then:

```sh
cd backend
docker compose -f compose.yaml -f compose.prod.yaml pull
docker compose -f compose.yaml -f compose.prod.yaml up -d
```

Pin `TWT_IMAGE_TAG` (e.g. `v1.0.0`) instead of tracking `latest`, and set
`TWT_IMAGE_PREFIX` when pulling from a fork's registry.

Compose is split by environment. `compose.dev.yaml` adds the dev JWKS mount
and the direct API on `127.0.0.1:9080/api/v1`; `compose.prod.yaml` adds the
TLS edge and requires real IdP values. The base stack keeps the same-origin
proxy on `127.0.0.1:8080`, which serves `/` and `/api/v1`. For a real domain,
see [`backend/README.md`](backend/README.md#real-domain): point DNS at the
host, free `80`/`443`, then either forward an existing TLS proxy to
`127.0.0.1:8080` or run the prod compose file with its `tls` service.

Auth is OIDC bearer (JWT). For local dev, generate your personal keypair:

```sh
cd backend && ./dev/gen-dev-keys.sh
```

This creates `dev/dev-key.pem`, `dev/jwks.json`, and `dev/token.txt`
(gitignored). Compose sets `I_ACCEPT_DEV_JWKS=1` for you; `cargo run` needs it
in the environment when `OAUTH_JWKS_FILE` is set.

## Quick start (web)

```sh
# from the repo root — no build step, plain ES modules
python3 -m http.server 5173 --bind 127.0.0.1 --directory web
```

Or run it from the container image / the full compose stack:

```sh
docker build -t twt-web web
docker run -d --name twt-web -p 127.0.0.1:5173:80 \
  -e TWT_CSP_CONNECT_SRC='http://localhost:* http://127.0.0.1:*' twt-web
cd backend && docker compose -f compose.yaml -f compose.dev.yaml up -d --build   # same-origin stack
```

The compose proxy is published on loopback `127.0.0.1:8080`; a
TLS-terminating reverse proxy is the public listener and owns the
HTTP→HTTPS redirect and HSTS. The browser sees one origin, so the compose
stack needs no CORS; set `PUBLIC_URL=https://twt.example.com` only when the
frontend has its own origin.

For the compose stack, open `http://localhost:8080/`; the same-origin API base
is `http://localhost:8080/api/v1` and the login form pre-fills it from the page
origin. For the standalone dev server on 5173, use
the devenv API (`http://localhost:8080/api/v1`) and register
`http://127.0.0.1:5173/` as the redirect URI; the devenv defaults allow that
origin. The browser runs the same OAuth bootstrap + discovery + PKCE flow as
the app. See [`web/README.md`](web/README.md).

## Development (devenv)

This repo has a [devenv](https://devenv.sh)-based dev environment:

```sh
devenv --profile backend up -d     # Postgres 16 + the API as a managed process
devenv --profile backend shell     # Rust toolchain + Postgres client, Node 22
```

The API then listens on `127.0.0.1:8080/api/v1`, Postgres on
`127.0.0.1:5432`, with DSN/env sourced from `backend/dev/` (run
`backend/dev/gen-dev-keys.sh` once).

First run in a fresh clone: `devenv update` (regenerates `devenv.lock`), then
activate the profile as above. Requires
[devenv 2.2+](https://devenv.sh/getting-started/).

## Tests

```sh
cd web && node --test                      # web unit tests (Node 22+)

cd backend
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked          # needs Postgres; see backend/README.md
npx --yes @redocly/cli lint docs/openapi.yaml   # from the repo root
```

CI (`.github/workflows/`) runs `backend-tests`, `web-tests`, `docker` (builds
all images and boots the dev stack), `openapi-lint` and `secret-scan` on every
push and pull request; `docker-publish` pushes the multi-arch
`ghcr.io/<owner>/{twt-cloud,twt-web,twt-proxy}` images on `main` and `v*` tags.
See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

The backend and `docs/` are **GPL-3.0-or-later** — see
[`backend/LICENSE`](backend/LICENSE) (also declared
`license = "GPL-3.0-or-later"` in `backend/Cargo.toml`).

The web frontend is **GPL-3.0-only** — its engine is a port of the Android
app's code — see [`web/LICENSE`](web/LICENSE). Its files carry SPDX headers;
where a component LICENSE or header names a license, that one applies.

The full GPL-3.0 text is also at [`LICENSE`](LICENSE).
