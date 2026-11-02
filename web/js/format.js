// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Local-time helpers for the web frontend.
//
// All day boundaries and clock labels use the browser's time zone — the
// equivalent of the app's "home time zone" for a browser running where the
// user works. The calculation engine receives a DST-aware offset provider.

import { formatMinutes, todayEpochDay } from "./engine.js";

export { formatMinutes };

function pad2(value) {
    return String(value).padStart(2, "0");
}

/** DST-aware east-positive UTC offset in minutes for a given instant. */
export function homeOffsetFor(ms) {
    return -new Date(ms).getTimezoneOffset();
}

/** Local epoch day (days since 1970-01-01 in local time). */
export function localEpochDay(ms) {
    return todayEpochDay(ms, homeOffsetFor(ms));
}

export function mondayOf(epochDay) {
    return epochDay - ((epochDay + 3) % 7);
}

/** "YYYY-MM-DD" of a local date. */
export function localDateISO(ms) {
    const d = new Date(ms);
    return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** "HH:MM" of a local time. */
export function localTimeHHMM(ms) {
    const d = new Date(ms);
    return `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** Clock label in the browser's locale (24h where the locale uses it). */
export function formatClock(ms) {
    if (ms === null || ms === undefined) return "";
    return new Date(ms).toLocaleTimeString(undefined, {
        hour: "2-digit",
        minute: "2-digit",
    });
}

/** Date + time in one line, e.g. "27 Oct 2026, 14:05". */
export function formatDateTime(ms) {
    if (ms === null || ms === undefined) return "";
    return new Date(ms).toLocaleString(undefined, {
        day: "2-digit",
        month: "short",
        year: "numeric",
        hour: "2-digit",
        minute: "2-digit",
    });
}

/** Short weekday + day + month label for a local date. */
export function formatDateLabel(ms) {
    return new Date(ms).toLocaleDateString(undefined, {
        weekday: "short",
        day: "2-digit",
        month: "short",
    });
}

export function formatWeekday(epochDay) {
    return new Date(epochDay * 86400000).toLocaleDateString(undefined, {
        weekday: "short",
        timeZone: "UTC",
    });
}

export function formatEpochDayShort(epochDay) {
    return new Date(epochDay * 86400000).toLocaleDateString(undefined, {
        day: "2-digit",
        month: "short",
        timeZone: "UTC",
    });
}

/**
 * Combines date (`YYYY-MM-DD`) and time (`HH:MM`) inputs into epoch seconds
 * and the UTC offset (minutes) at that local instant.
 */
export function inputDateTimeToEpoch(dateISO, timeHHMM) {
    const dateParts = String(dateISO).split("-").map(Number);
    const timeParts = String(timeHHMM).split(":").map(Number);
    if (
        dateParts.length !== 3
        || timeParts.length !== 2
        || ![...dateParts, ...timeParts].every(Number.isInteger)
        || dateParts[1] < 1 || dateParts[1] > 12
        || dateParts[2] < 1 || dateParts[2] > 31
        || timeParts[0] < 0 || timeParts[0] > 23
        || timeParts[1] < 0 || timeParts[1] > 59
    ) {
        throw new Error("Invalid date or time");
    }
    const [year, month, day] = dateParts;
    const [hours, minutes] = timeParts;
    const d = new Date(year, month - 1, day, hours, minutes, 0, 0);
    // `Date` silently normalizes nonexistent wall-clock times: on a
    // spring-forward day 02:30 becomes 03:30. Never store a time the user
    // did not enter. (On a fall-back day both 02:30 instances round-trip;
    // `Date` resolves to the first one, which is accepted.)
    if (
        isNaN(d.getTime())
        || d.getFullYear() !== year
        || d.getMonth() !== month - 1
        || d.getDate() !== day
        || d.getHours() !== hours
        || d.getMinutes() !== minutes
    ) {
        throw new Error("That time does not exist on this date (check the date and daylight-saving changes)");
    }
    return {
        time: Math.floor(d.getTime() / 1000),
        zoneOffset: -d.getTimezoneOffset(),
        date: d,
    };
}

/** Duration input ("8:30" or "8") in minutes; throws when malformed. */
export function parseDurationInput(value) {
    const raw = String(value ?? "").trim();
    if (raw === "") {
        throw new Error("Enter a duration like 8:30");
    }
    const match = raw.match(/^(-?)(\d{1,3})(?:[:.](\d{1,2}))?$/);
    if (!match) {
        throw new Error(`"${raw}" is not a duration (use H:MM)`);
    }
    const hours = Number(match[2]);
    const minutes = match[3] === undefined ? 0 : Number(match[3]);
    if (minutes > 59) {
        throw new Error("Minutes must be between 00 and 59");
    }
    const total = hours * 60 + minutes;
    return match[1] === "-" ? -total : total;
}

