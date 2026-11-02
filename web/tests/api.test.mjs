// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tests for the REST client: URL building, the 401 retry, RFC 9457 errors and
// the two pagination loops (events by offset, targets by date).

import { test } from "node:test";
import assert from "node:assert/strict";

import { ApiClient, ApiError } from "../js/api.js";

const API = "http://api.test/api/v1";

function json(body, status = 200, headers = {}) {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json", ...headers },
    });
}

function fakeAuth(token = "t0") {
    return {
        token,
        refreshes: 0,
        async accessToken() {
            return this.token;
        },
        async forceRefresh() {
            this.refreshes += 1;
            this.token = `t${this.refreshes}`;
        },
    };
}

function client(fetchImpl, auth = fakeAuth()) {
    return new ApiClient({ apiBase: `${API}/`, auth, fetchImpl });
}

test("builds query strings and drops empty values", async () => {
    const seen = [];
    const api = client(async (url) => {
        seen.push(url);
        return json([]);
    });
    await api.get("/events", {limit: 1000, offset: 0, cursor: "", missing: null});
    assert.deepEqual(seen, [`${API}/events?limit=1000&offset=0`]);
});

test("retries once with a refreshed token after a 401", async () => {
    const auth = fakeAuth();
    const headers = [];
    const api = client(async (_url, init) => {
        headers.push(init.headers.Authorization);
        return headers.length === 1 ? json({title: "unauthorized"}, 401) : json([{id: "1"}]);
    }, auth);

    assert.deepEqual(await api.listTasks(), [{id: "1"}]);
    assert.equal(auth.refreshes, 1);
    assert.deepEqual(headers, ["Bearer t0", "Bearer t1"]);
});

test("a second 401 is reported instead of retried forever", async () => {
    const auth = fakeAuth();
    const api = client(async () => json({title: "unauthorized", detail: "expired"}, 401), auth);
    await assert.rejects(api.listTasks(), (error) => {
        assert.ok(error instanceof ApiError);
        assert.equal(error.status, 401);
        assert.equal(error.message, "expired");
        return true;
    });
    assert.equal(auth.refreshes, 1);
});

test("returns null for 204 and text for non-JSON responses", async () => {
    const api = client(async () => new Response(null, {status: 204}));
    assert.equal(await api.deleteTask("t1", "2026-10-27T00:00:00Z"), null);

    const textApi = client(async () => new Response("csv,data", {
        status: 200,
        headers: { "content-type": "text/csv" },
    }));
    assert.equal(await textApi.get("/export"), "csv,data");
});

test("surfaces RFC 9457 problem details", async () => {
    const api = client(async () => json(
        {title: "Conflict", detail: "event was modified", status: 409},
        409,
        { "content-type": "application/problem+json" },
    ));
    await assert.rejects(api.deleteEvent("e1", "stale"), (error) => {
        assert.equal(error.status, 409);
        assert.equal(error.message, "event was modified");
        assert.equal(error.problem.title, "Conflict");
        return true;
    });
});

test("network failures become ApiError status 0", async () => {
    const api = client(async () => {
        throw new Error("connection refused");
    });
    await assert.rejects(api.get("/tasks"), (error) => {
        assert.ok(error instanceof ApiError);
        assert.equal(error.status, 0);
        assert.match(error.message, /connection refused/);
        return true;
    });
});

test("listAllEvents pages by offset until a short page", async () => {
    const all = Array.from({length: 2500}, (_, i) => ({id: `e${i}`}));
    const calls = [];
    const api = client(async (url) => {
        const params = new URL(url).searchParams;
        const limit = Number(params.get("limit"));
        const offset = Number(params.get("offset"));
        calls.push({limit, offset});
        return json(all.slice(offset, offset + limit));
    });

    const events = await api.listAllEvents();
    assert.equal(events.length, 2500);
    assert.deepEqual(calls.map((c) => c.offset), [0, 1000, 2000]);
    assert.ok(calls.every((c) => c.limit === 1000));
});

test("listAllTargets pages by offset until a short page", async () => {
    const all = Array.from({length: 2500}, (_, i) => ({id: `t${i}`, date: "2026-01-01"}));
    const calls = [];
    const api = client(async (url) => {
        const params = new URL(url).searchParams;
        const limit = Number(params.get("limit"));
        const offset = Number(params.get("offset"));
        calls.push({limit, offset});
        return json(all.slice(offset, offset + limit));
    });

    const targets = await api.listAllTargets();
    assert.equal(targets.length, 2500);
    assert.equal(new Set(targets.map((t) => t.id)).size, 2500);
    assert.deepEqual(calls.map((c) => c.offset), [0, 1000, 2000]);
    assert.ok(calls.every((c) => c.limit === 1000));
});

test("listAllTargets refuses an unbounded history instead of looping", async () => {
    let calls = 0;
    const api = client(async () => {
        calls += 1;
        return json(Array.from({length: 1000}, (_, i) => ({id: `t${calls}-${i}`})));
    });

    await assert.rejects(api.listAllTargets(), /more than 50000 targets/);
    assert.equal(calls, 50);
});
