// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tests for the DOM helpers that decide what reaches innerHTML, and for the
// error text shown to the user.

import { test } from "node:test";
import assert from "node:assert/strict";

import { escapeHtml, formatError } from "../js/ui.js";
import { ApiError } from "../js/api.js";

test("escapeHtml neutralises every HTML metacharacter", () => {
    assert.equal(
        escapeHtml(`<img src=x onerror="alert('x')">&`),
        "&lt;img src=x onerror=&quot;alert(&#39;x&#39;)&quot;&gt;&amp;",
    );
    assert.equal(escapeHtml("a &amp; b"), "a &amp;amp; b");
    assert.equal(escapeHtml(null), "");
    assert.equal(escapeHtml(undefined), "");
    assert.equal(escapeHtml(0), "0");
});

test("formatError prefers the problem detail and keeps the status", () => {
    const error = new ApiError(
        409,
        {title: "Conflict", detail: "event was modified", status: 409},
        "Conflict",
    );
    assert.equal(formatError(error), "event was modified (HTTP 409)");
    assert.equal(formatError(new ApiError(400, {title: "Bad request"})), "Bad request (HTTP 400)");
    assert.equal(formatError(new Error("boom")), "boom");
    assert.equal(formatError(null), "Unknown error");
});
