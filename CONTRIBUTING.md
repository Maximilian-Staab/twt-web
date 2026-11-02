<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Contributing

Thanks for helping with twt-web — the self-hosted sync backend (`twt-cloud`)
and the browser frontend for TrackWorkTime. The Android app and the
DankMaterialShell plugin live in their own repositories.

## Layout and test commands

| component | path | language | tests | license |
| --- | --- | --- | --- | --- |
| storage backend | `backend/` | Rust (Axum + sqlx) | see below | GPL-3.0-or-later |
| web frontend | `web/` | plain ES modules, no build | `cd web && node --test` | GPL-3.0-only |
| contract | `docs/` | OpenAPI 3.1 + Markdown | `npx --yes @redocly/cli lint docs/openapi.yaml` | GPL-3.0-or-later |

Backend tests need Postgres; CI runs them against a service container:

```sh
# with a running Postgres and the dev keypair generated (see backend/README.md)
cd backend
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo deny check        # cargo install cargo-deny
```

`Cargo.lock` is committed: keep `--locked` on every cargo command so local and
CI builds resolve the same versions. `rust-version = "1.89"` is enforced by the
`msrv` CI job, so raise it in the same change that needs a newer toolchain.

`devenv --profile backend shell` provides the Rust toolchain, Postgres and the
dev keys. See the "Development (devenv)" section of the
[root README](README.md).

## Contracts and duplicated calculations

- `docs/openapi.yaml` is the normative REST contract. Every client (this
  backend's tests, the web frontend, the Android app, the DMS plugin) must
  match it; behavior changes update the OpenAPI document in the same change.
- `docs/DESIGN.md` describes the storage model and the sync protocol.
- Work-time math exists in the app's `TimeCalculatorV2` / `TimerManager` and
  `web/js/engine.js`. The DMS plugin in the separate `twt-dms-plugin` repo
  carries its own `TwtEngine.js` port. When changing shared semantics, update
  both ports and their documented differences together; the cross-repo parity
  test was removed with the plugin, so this is a release checklist item.
- The web client stays build-free: no bundler, no npm dependencies, no
  generated files. It must keep working when served by any static file server.

## Style

- Every new file starts with an SPDX header and a copyright line:
  `SPDX-License-Identifier: GPL-3.0-or-later` for backend/docs, and
  `GPL-3.0-only` for `web/` (it derives from the upstream TrackWorkTime code).
  HTML uses `<!-- ... -->`, CSS `/* ... */`.
- Match the surrounding code: 4-space indentation in JS/CSS/HTML, `var`-style
  ES5 in the engine port (it is shared with the plugin's QML port), rustfmt
  defaults in the backend.
- The web client's CSP forbids inline scripts and event-handler attributes;
  build DOM through `escapeHtml`/`textContent` (see `web/js/ui.js`).
- User-facing web strings are English.

## Commits and pull requests

- Commit subjects use the prefixes already in the history: `feat(...)`,
  `fix(...)`, `docs(...)`, `test(...)`, `chore(...)`, optionally with a `wip`
  marker for checkpoints.
- Keep a change scoped to one component where possible; say which test command
  you ran in the pull request.
- CI must be green: `backend-tests`, `web-tests`, `docker`, `openapi-lint`
  and `secret-scan` (`.github/workflows/`). `docker-publish` runs only on
  `main` and `v*` tags.

## Secrets and security

- Never commit keys, tokens or deployment config: `backend/dev/*` (except
  `gen-dev-keys.sh`) and `backend/.env` are gitignored on purpose.
- Report vulnerabilities as described in [SECURITY.md](SECURITY.md), not in a
  public issue.

## Licensing of contributions

There is no CLA. By submitting a change you agree to license it under the
license of the component you touch.
