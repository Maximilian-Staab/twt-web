// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// DST regression tests for inputDateTimeToEpoch. They run in their own test
// file (Node runs each file in a separate process), so the timezone change
// cannot leak into the other suites.

process.env.TZ = "Europe/Berlin";

import { test } from "node:test";
import assert from "node:assert/strict";

import { inputDateTimeToEpoch } from "../js/format.js";

test("a wall-clock time in the spring-forward gap is rejected", () => {
    assert.throws(
        () => inputDateTimeToEpoch("2026-03-29", "02:30"),
        /does not exist/,
    );
});

test("times on either side of the spring-forward gap keep their offset", () => {
    const before = inputDateTimeToEpoch("2026-03-29", "01:30");
    const after = inputDateTimeToEpoch("2026-03-29", "03:30");
    assert.equal(before.date.getHours(), 1);
    assert.equal(before.zoneOffset, 60);
    assert.equal(after.date.getHours(), 3);
    assert.equal(after.zoneOffset, 120);
});

test("a repeated fall-back time resolves to the first occurrence", () => {
    const fold = inputDateTimeToEpoch("2026-10-25", "02:30");
    assert.equal(fold.date.getDate(), 25);
    assert.equal(fold.date.getHours(), 2);
    assert.equal(fold.date.getMinutes(), 30);
    assert.equal(fold.zoneOffset, 120);
});

test("impossible calendar dates are rejected instead of rolling over", () => {
    assert.throws(() => inputDateTimeToEpoch("2026-02-30", "09:00"), /does not exist/);
});
