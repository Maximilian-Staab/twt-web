// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tests for the default API base: same-origin behind a reverse proxy, the
// :8080 API only on the documented dev server.

import { test } from "node:test";
import assert from "node:assert/strict";

import { DEV_API_BASE, defaultApiBase } from "../js/settings.js";

test("defaults to same-origin behind a reverse proxy", () => {
    assert.equal(
        defaultApiBase({origin: "https://twt.example.com", port: ""}),
        "https://twt.example.com/api/v1",
    );
    assert.equal(
        defaultApiBase({origin: "http://localhost:8080", port: "8080"}),
        "http://localhost:8080/api/v1",
    );
});

test("the dev server on :5173 keeps the :8080 API", () => {
    assert.equal(
        defaultApiBase({origin: "http://127.0.0.1:5173", port: "5173"}),
        DEV_API_BASE,
    );
    assert.equal(
        defaultApiBase({origin: "http://localhost:5173", port: "5173"}),
        DEV_API_BASE,
    );
});

test("falls back to the dev API without a usable location", () => {
    assert.equal(defaultApiBase(undefined), DEV_API_BASE);
    assert.equal(defaultApiBase(null), DEV_API_BASE);
    // file:// pages report the opaque origin "null"
    assert.equal(defaultApiBase({origin: "null", port: ""}), DEV_API_BASE);
});
