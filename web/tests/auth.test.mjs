// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tests for the pure helpers of the OAuth client. The browser round-trip
// itself (redirect, PKCE exchange, refresh) needs a window and an IdP, so it
// is covered by the manual sign-in check, not here.

import { test } from "node:test";
import assert from "node:assert/strict";

import { normalizeApiBase, readClaim, AuthError } from "../js/auth.js";

function idToken(payload) {
    return `header.${Buffer.from(JSON.stringify(payload), "utf8").toString("base64url")}.signature`;
}

test("normalizeApiBase accepts a server root or an API base", () => {
    assert.equal(normalizeApiBase("http://localhost:8080"), "http://localhost:8080/api/v1");
    assert.equal(normalizeApiBase("http://[::1]:8080"), "http://[::1]:8080/api/v1");
    assert.equal(normalizeApiBase("http://0.0.0.0:8080"), "http://0.0.0.0:8080/api/v1");
    assert.equal(normalizeApiBase("127.0.0.1:9080"), "http://127.0.0.1:9080/api/v1");
    assert.equal(normalizeApiBase("https://twt.example.com/"), "https://twt.example.com/api/v1");
    assert.equal(normalizeApiBase("https://twt.example.com/api/v1/"), "https://twt.example.com/api/v1");
    assert.equal(normalizeApiBase("https://twt.example.com/prefix"), "https://twt.example.com/prefix/api/v1");
    assert.equal(normalizeApiBase("  http://localhost:8080/api/v1  "), "http://localhost:8080/api/v1");
});

test("normalizeApiBase refuses an empty backend URL", () => {
    for (const value of ["", "   ", null, undefined]) {
        assert.throws(() => normalizeApiBase(value), AuthError);
    }
});

test("normalizeApiBase rejects protocol-relative, non-HTTP and malformed URLs", () => {
    for (const value of [
        "//evil.example/api/v1",
        "ftp://example.com/api/v1",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "https://",
        "https://host:not-a-port/api/v1",
    ]) {
        assert.throws(() => normalizeApiBase(value), AuthError, `accepted ${value}`);
    }
});

test("normalizeApiBase rejects credentials, queries and fragments", () => {
    for (const value of [
        "https://user:secret@example.com/api/v1",
        "https://user@example.com/api/v1",
        "https://example.com/api/v1?x=1",
        "https://example.com/api/v1#fragment",
    ]) {
        assert.throws(() => normalizeApiBase(value), AuthError, `accepted ${value}`);
    }
});

test("normalizeApiBase refuses plaintext HTTP for a remote host", () => {
    assert.throws(() => normalizeApiBase("http://example.com"), /https:/);
    assert.throws(() => normalizeApiBase("http://127.0.0.1.evil.example"), /https:/);
    assert.equal(normalizeApiBase("http://evil.localhost"), "http://evil.localhost/api/v1");
});

test("readClaim decodes UTF-8 claims without verification", () => {
    const token = idToken({sub: "user-alice", nonce: "abc123", name: "Grüße"});
    assert.equal(readClaim(token, "sub"), "user-alice");
    assert.equal(readClaim(token, "nonce"), "abc123");
    assert.equal(readClaim(token, "name"), "Grüße");
    assert.equal(readClaim(token, "missing"), "");
});

test("readClaim returns an empty string for unusable tokens", () => {
    assert.equal(readClaim("", "sub"), "");
    assert.equal(readClaim("not-a-jwt", "sub"), "");
    assert.equal(readClaim("a.%%%.c", "sub"), "");
    assert.equal(readClaim(idToken({sub: 42}), "sub"), "");
});
