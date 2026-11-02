<!-- SPDX-License-Identifier: GPL-3.0-only -->

# twt web frontend

The browser client of [twt-web](../README.md): a small, build-free web UI for the
twt-cloud storage backend. It mirrors the minimal functionality of the Android
app and the twtBar DMS plugin:

| area | what it does |
| --- | --- |
| Sign-in | OAuth 2.0 Authorization Code + PKCE, same bootstrap (`GET /config` → OIDC discovery) and refresh-token flow as `CloudAuthManager` / `twt_auth.py`; discovery and token grants are relayed by the backend for browser CORS |
| Dashboard | clocked-in state, worked today, today's target, flexi balance, clock in/out with task + comment, week table (in/out/worked/target/balance), today's events |
| Events | list by range, add, edit, delete (`CLOCK_IN` / `CLOCK_OUT`) |
| Tasks | create, rename, enable/disable, make default, delete (same "in use"/"last task" guards as the app) |
| Targets | create, edit, delete day targets, grants, ignores and flexi targets |
| Settings | flexi start/target, reset interval, workdays, account/server info, sign out |

The calculations are a port of `TimeCalculatorV2` / `TimerManager` — see
`js/engine.js`. The web copy differs intentionally: it reports today's target,
keeps accounting for elapsed targets when there are no events (so the
dashboard matches the week table), freezes the displayed balance on
`DAY_IGNORE` like the app, and reports week totals as row sums (stable across a
flexi reset). The DMS plugin in the separate `twt-dms-plugin` repository has
its own engine port.

## Running

From the repo root:

```sh
python3 -m http.server 5173 --bind 127.0.0.1 --directory web
```

Open <http://127.0.0.1:5173/>. Any static file server works; there is no build
step and no runtime dependency (plain ES modules).

### Docker

```sh
# prebuilt (published by CI for amd64 and arm64)
docker run -d --name twt-web -p 127.0.0.1:5173:80 \
  -e TWT_CSP_CONNECT_SRC='http://localhost:* http://127.0.0.1:*' \
  ghcr.io/maximilian-staab/twt-web:latest

# or build it yourself
docker build -t twt-web web
docker run -d --name twt-web -p 127.0.0.1:5173:80 \
  -e TWT_CSP_CONNECT_SRC='http://localhost:* http://127.0.0.1:*' twt-web
```

The image is plain Caddy over `index.html`, `styles.css` and `js/`; tests,
`README.md` and `package.json` are not copied. Health endpoint: `/healthz`.

The container is an HTTP origin, not an edge. Run a TLS-terminating reverse
proxy in front of it (and the API); the proxy is the only public listener and
owns the HTTP→HTTPS redirect and HSTS. Do not publish the container port
directly.

Caddy sends a CSP header. `TWT_CSP_CONNECT_SRC` is appended to its
`connect-src` (`'self'` is always allowed); set it to the public API/IdP
origins. The compose stack is same-origin, so it leaves this empty; set it
for a cross-origin API/IdP. A standalone container needs it for a cross-origin
backend. A same-origin API behind the proxy is covered by `'self'`.

The compose stack in `backend/compose.yaml` includes this service and a
same-origin proxy, so from `backend/`:

```sh
docker compose -f compose.yaml -f compose.dev.yaml up -d --build
```

The proxy is published on `127.0.0.1:8080`: `/` goes to the web container and
`/api/v1` to the API, so the browser sees one origin. `compose.dev.yaml` adds
the dev JWKS setup and the direct API on `127.0.0.1:9080/api/v1` for
CLI/plugin clients; `compose.prod.yaml` replaces that with the TLS edge.

Open `http://localhost:8080/`; the same-origin API base is
`http://localhost:8080/api/v1`. No CORS is needed. `PUBLIC_URL` (or
`CORS_ALLOWED_ORIGINS`) is only for a frontend on a separate origin.

For a real domain, point an existing TLS terminator at `127.0.0.1:8080`, or
enable the compose `tls` service in `backend/compose.prod.yaml` (Caddy
certificates + HSTS); see
[`backend/README.md`](../backend/README.md#real-domain). Register
`https://twt.example.com/` as the redirect URI.

The login form pre-fills the backend URL with `<page origin>/api/v1`, so a
same-origin deployment (compose proxy, TLS terminator, Pangolin) needs no
input. Enter the server root (`https://twt.example.com`) or any API base
(`https://twt.example.com/api/v1`) to point the page elsewhere; the value is
stored. The one exception is the standalone dev server on 5173, whose default
stays the devenv API (`http://localhost:8080/api/v1`) and its CORS defaults —
that page origin has no API of its own.

To offer a backend in one link, pass `api` as a query parameter — useful when
the page is served locally but the API is remote:

```
http://127.0.0.1:5173/?api=https://twt.example.com/api/v1
```

The link is untrusted input: the page names the requested host in a
confirmation dialog and keeps the stored server unless you accept. A confirmed
value is persisted. The OAuth redirect URI remains the page URL without the
query.

Remote backends must use `https://`; plain `http://` is accepted only for
`localhost`, `127.0.0.0/8`, `[::1]`, and `*.localhost`. URLs with a user name,
password, query string or fragment are rejected.

## Tests

The client is plain ES modules with no runtime dependency, so the tests run on
Node 22+ without installing anything:

```sh
cd web
node --test        # unit tests
```

CI (`.github/workflows/web-tests.yaml`) additionally syntax-checks every module
with `node --check`. `tests/engine.test.mjs` covers the shared calculations and
the web-only additions; `tests/api.test.mjs` drives the paging loops and the
401 retry with an injected `fetch`.

## Browser support

Evergreen browsers only: the client needs ES modules, private class fields,
`<dialog>` and `color-mix()` (Chromium 111+, Firefox 113+, Safari 16.2+), plus
Web Crypto for PKCE, which requires a secure context — `localhost` or HTTPS.

## Backend CORS

A browser cannot call the API cross-origin unless the backend allows it. Set
`CORS_ALLOWED_ORIGINS` on the backend to a comma-separated list of exact
origins (or `*`):

```sh
CORS_ALLOWED_ORIGINS=http://127.0.0.1:5173,http://localhost:5173
```

The devenv backend profile defaults to the two localhost:5173 origins for the
standalone dev server. The compose stack is same-origin (proxy on 8080), so it
sets no CORS allowlist by default; use `PUBLIC_URL` or `CORS_ALLOWED_ORIGINS`
only when the frontend is served from another origin. A same-origin
deployment (static files and `/api/v1` behind one reverse proxy) needs no CORS
configuration at all.

## OAuth provider setup

The web client is an OAuth **public client** (no secret) and registers no
backend callback. It reuses the `clientId` advertised by `GET /api/v1/config`,
so the IdP client used by the app/plugin can be reused when it supports another
redirect URI:

1. Allow the Authorization Code flow with PKCE (`S256`).
2. Register this deployment's page URL as a redirect URI, exactly — the page
   strips `index.html`, so it is usually the directory URL:
   - `http://127.0.0.1:5173/` (standalone dev server)
   - `http://localhost:8080/` (compose same-origin stack)
   - `https://twt.example.com/` (or whatever path the files are served from)

   `localhost` and `127.0.0.1` are different origins for both the redirect URI
   and CORS. Open the page at whichever spelling the provider registered; the
   login screen prints the exact URI it will send.
3. Allow CORS from the web origin on the **token endpoint** (the browser posts
   the code/refresh grant directly). Without it, sign-in fails with
   “token endpoint unreachable … must allow CORS”.
4. Keep `offline_access` in the scopes (the frontend requests it if missing) so
   the session can refresh without a new login.
5. Set the backend's `OAUTH_ISSUER_URL`, `OAUTH_AUDIENCE`, `OAUTH_CLIENT_ID`,
   `OAUTH_JWKS_URL` as usual.

The web frontend normally fetches the discovery document through
`GET /api/v1/oidc-config`: the backend proxies
`<issuer without trailing "/">/.well-known/openid-configuration`, because an
IdP does not necessarily send CORS headers on that document (authentik does
not). Against an older backend without the endpoint it falls back to fetching
the issuer directly.

The **token exchange and refresh grant** go through
`POST /api/v1/oauth/token` when `/config` advertises `tokenProxy: true`: the
backend forwards the grant to the discovered token endpoint, so the IdP does
not need CORS on it (authentik does not send CORS headers on the actual token
response). Against an older backend without the relay, the browser posts to
the IdP directly and the provider must allow CORS there.

## Storage and security

- Tokens live in `localStorage` (`twt.auth.v1`) and are refreshed shortly
  before expiry; a 401 forces one refresh and then signs out.
- The in-flight PKCE verifier/state lives in `sessionStorage` and is cleared on
  the callback.
- Flexi settings live in `localStorage` (`twt.settings.v1`) and are browser-local,
  like the twtBar plugin settings; they are not written to the backend
  preference store.
- Signing out discards tokens. Run the container behind a TLS-terminating
  reverse proxy outside localhost; the proxy owns the HTTP→HTTPS redirect and
  HSTS. Do not publish the container port directly.
- `index.html` sets a fallback CSP (permissive `connect-src` for header-less
  dev servers). The container's Caddy sends a stricter header CSP: same-origin
  scripts, `frame-ancestors 'none'`, and `connect-src 'self'` plus
  `TWT_CSP_CONNECT_SRC`. Set that variable to the public API/IdP origins. It
  also means `?api=` can only reach origins listed there, so a fixed public
  deployment should pin the backend. The bearer token is still readable by any
  same-origin script, so keep the deployment free of untrusted scripts/plugins.
- `?api=<url>` asks the app to use another backend. A link can still point this
  page at a server you do not control — and with the token relay, that server
  would receive the authorization code and PKCE verifier. The page therefore
  names the requested host in a confirmation dialog, focuses “Keep current
  server”, and persists the value only when you accept. Treat an unexpected
  `api` link like any other credential request and check the host.

## Notes

- All times use the browser's time zone, including day boundaries (DST-aware).
- The client fetches the full event and target history so the flexi balance
  uses the same day-window semantics as the app and plugin; it is a
  personal-data-sized app, not an analytics tool. Events and targets are
  fetched with `limit`/`offset` pages and capped at 50 000 rows each.
- A 60-second timer refetches while the tab is visible, so a long history is
  downloaded again on every tick. A refresh that lands while a field in the
  current view has focus is deferred until the field loses focus, so typing is
  not interrupted by the re-render.
- Event PATCH is partial on the wire, but the current backend treats `null` as
  “leave unchanged” for `task`/`comment`; clearing an existing task reference is
  therefore not offered by the edit dialog.

## License

GPL-3.0-only — see [LICENSE](LICENSE) in this directory. The per-file SPDX
headers take precedence over the license summary in the root README.

`js/engine.js` is derived from TrackWorkTime's `TimeCalculatorV2` /
`TimerManager` (Copyright Mathis DT and contributors, GPL-3.0); `js/auth.js`
mirrors the app's `CloudAuthManager` OAuth flow.
