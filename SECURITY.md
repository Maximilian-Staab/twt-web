<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Security policy

twt-web is the self-hosted part of TrackWorkTime: a storage backend and a web
frontend that you run yourself, authenticated with your own OIDC provider.

## Supported versions

There are no release branches. Security fixes land on `main`; run a current
checkout or commit.

## Reporting a vulnerability

Please report privately — do not open a public issue for a suspected
vulnerability.

- Preferred: GitHub private vulnerability reporting on
  [Maximilian-Staab/twt-web](https://github.com/Maximilian-Staab/twt-web/security/advisories/new).
- If that is unavailable, contact the maintainer shown on the repository
  profile.

Include the affected component (backend or web), the commit or version,
reproduction steps, expected vs. actual behavior, and the impact you see. Give
us a chance to ship a fix before public disclosure.

## Scope

In scope:

- Backend: JWT validation (issuer, audience, algorithm pinning), the
  unauthenticated OAuth relays (`/oidc-config`, `/oauth/token`), authorization
  between users, the sync change log and cursor handling, CSV/snapshot
  import/export, CORS handling.
- Web frontend: token handling and storage, PKCE/state/nonce handling, XSS via
  rendered API data, CSP bypasses, backend/IdP origin handling.

Out of scope: the Android app and the DMS plugin (report those in their own
repositories), configuration of your IdP or reverse proxy, browser extensions,
and the deployment hardening items below (report only if the code defeats
them).

## Known design decisions

These are deliberate; report them only if you can bypass them.

- The web client keeps tokens in `localStorage` and ships a CSP that allows
  same-origin scripts only. Any same-origin script can read the token, so
  deployments must not serve untrusted code.
- Signing out discards tokens locally; the refresh token is not revoked at the
  IdP.
- CORS is opt-in via `CORS_ALLOWED_ORIGINS`; `*` is supported but discouraged
  for public deployments.
- The backend accepts a static dev JWKS only when `I_ACCEPT_DEV_JWKS=1`, which
  `compose.dev.yaml` sets for local development.

## Deployment hardening

- Serve the backend and the web frontend over HTTPS; Web Crypto (PKCE) needs a
  secure context.
- Set `CORS_ALLOWED_ORIGINS` (or `PUBLIC_URL` in compose) to exact origins
  when the frontend has its own origin; the same-origin compose proxy needs
  none.
- Add security headers at the static host or TLS terminator. The container's
  Caddy sends a CSP header, including `frame-ancestors 'none'`; `index.html`
  keeps a meta fallback for header-less dev servers, which cannot express
  `frame-ancestors`. Send at least:
  - `Content-Security-Policy: default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`
  - `X-Content-Type-Options: nosniff`
  - `Referrer-Policy: no-referrer`
  - `Strict-Transport-Security` on HTTPS
- Register the exact redirect URI of the web deployment on the OIDC client, and
  keep `offline_access` scoped to clients you trust.
- Keep `backend/.env` and the generated dev keys out of version control, and
  rotate them if they leak.
