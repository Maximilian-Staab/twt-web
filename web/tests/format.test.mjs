// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tests for the local-time/format helpers used by every view.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
    homeOffsetFor,
    inputDateTimeToEpoch,
    localDateISO,
    localEpochDay,
    localTimeHHMM,
    mondayOf,
    parseDurationInput,
} from "../js/format.js";
import { todayEpochDay } from "../js/engine.js";

test("parseDurationInput accepts H:MM, H and negative values", () => {
    assert.equal(parseDurationInput("8:30"), 510);
    assert.equal(parseDurationInput("8"), 480);
    assert.equal(parseDurationInput("08:05"), 485);
    assert.equal(parseDurationInput(" 0:00 "), 0);
    assert.equal(parseDurationInput("-1:30"), -90);
    assert.equal(parseDurationInput("40:00"), 2400);
});

test("parseDurationInput rejects malformed durations", () => {
    for (const value of ["", "   ", "abc", "8:60", "8:5:5", "1h30", "8:", ":30"]) {
        assert.throws(() => parseDurationInput(value), /duration|Minutes/, `accepted ${value}`);
    }
    assert.throws(() => parseDurationInput(undefined), /duration/);
});

test("inputDateTimeToEpoch keeps the entered wall-clock time", () => {
    const { time, zoneOffset } = inputDateTimeToEpoch("2026-10-27", "09:05");
    const back = new Date(time * 1000);
    assert.equal(back.getFullYear(), 2026);
    assert.equal(back.getMonth(), 9);
    assert.equal(back.getDate(), 27);
    assert.equal(back.getHours(), 9);
    assert.equal(back.getMinutes(), 5);
    assert.equal(back.getSeconds(), 0);
    assert.equal(zoneOffset, -back.getTimezoneOffset());
});

test("inputDateTimeToEpoch rejects unparsable input", () => {
    for (const [date, time] of [["", "09:00"], ["2026-1", "09:00"], ["2026-10-27", ""], ["2026-10-27", "9"]]) {
        assert.throws(() => inputDateTimeToEpoch(date, time), /Invalid date or time/, `${date} ${time}`);
    }
});

test("local date and time labels are zero-padded", () => {
    const ms = new Date(2026, 9, 7, 4, 5).getTime();
    assert.equal(localDateISO(ms), "2026-10-07");
    assert.equal(localTimeHHMM(ms), "04:05");
});

test("local days follow the browser offset", () => {
    const localNoon = new Date(2026, 9, 27, 12, 0).getTime();
    assert.equal(homeOffsetFor(localNoon), -new Date(localNoon).getTimezoneOffset());
    // The epoch day must name the same calendar date as the local formatter.
    const day = localEpochDay(localNoon);
    assert.equal(new Date(day * 86400000).toISOString().slice(0, 10), localDateISO(localNoon));
    assert.equal(todayEpochDay(localNoon, homeOffsetFor(localNoon)), day);

    const monday = mondayOf(day);
    assert.ok(monday <= day && day - monday < 7);
    assert.equal(mondayOf(monday), monday);
});
