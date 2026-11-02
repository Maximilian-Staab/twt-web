// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors
// Derived from TrackWorkTime (https://codeberg.org/mathisdt/trackworktime),
// Copyright (C) Mathis DT and contributors, licensed GPL-3.0.
//
// Work-time calculation for the web frontend.
//
// This is the same semantics as the twtBar plugin's `TwtEngine.js` (itself a
// port of the Android app's TimeCalculatorV2 / TimerManager). It is duplicated
// here because the QML plugin loads a `.pragma library` script that cannot be
// imported as an ES module; keep the two in sync when changing either.
//
// Differences from the plugin copy:
//   * ES-module exports instead of QML globals.
//   * `calculate()` also reports today's target and, with no events at all,
//     still accounts for the elapsed targets of the current week so the
//     dashboard balance agrees with the week table.
//   * `calculate()` freezes the displayed balance on DAY_IGNORE (the app's
//     `currentBalance` behavior; the plugin only documents it).
//   * `calculateWeek()` adds the per-day rows the week table needs; its
//     totals are the row sums (stable across a flexi reset) and null for a
//     week that has not started yet.
//
// Event shape  (GET /events):    { id, time (epoch sec), zoneOffset (min), type: "CLOCK_IN"|"CLOCK_OUT", task, comment, updatedAt, deletedAt }
// Target shape (GET /targets):   { id, date "YYYY-MM-DD", type: "DAY_SET"|"DAY_GRANT"|"DAY_IGNORE"|"FLEXI_SET"|"FLEXI_ADD", value (min), ... }

// settings = {
//   flexiEnabled:   bool,
//   flexiStartValue:"H:MM"   (minutes credited at the start of tracking),
//   flexiTarget:    "H:MM"   (target work per week),
//   workdays:       [bool x7] (Mon..Sun),
//   resetInterval:  "NONE"|"DAILY"|"WEEKLY"|"MONTHLY"|"QUARTERLY"|"HALF_YEARLY"|"YEARLY"
// }
// homeOffset: how local day boundaries are derived. Either a fixed number of
//   east-positive minutes, or a function (ms) => east-positive minutes.
export function calculate(events, targets, settings, nowMs, homeOffset) {
    var result = {
        workedToday: 0,
        todayTarget: 0,
        balance: 0,
        clockedIn: false,
        lastEventTime: 0,
        flexiUsed: settings.flexiEnabled
    };
    var offFn = resolveOffset(homeOffset);
    var dayTargets = buildDayTargets(targets);
    var normalWork = normalWorkMinutes(settings);
    var evs = normalizeEvents(events);
    var todayEpoch = todayEpochDay(nowMs, offFn(nowMs));

    // With no events at all, still account for the elapsed days of the
    // current week (the dashboard balance must agree with the week table).
    var firstDay = evs.length > 0
        ? todayEpochDay(evs[0].time * 1000, offFn(evs[0].time * 1000))
        : todayEpoch;
    // getTimesAt: start at the Monday of the first event's week, crediting
    // flexiStartValue. A future-only history still accounts for the current
    // week's elapsed targets (the app's main view starts there).
    var startDay = Math.min(mondayOf(firstDay), mondayOf(todayEpoch));
    var futureClockOutExists = hasFutureClockOut(evs, nowMs);

    var actual = settings.flexiEnabled ? parseHoursMinutes(settings.flexiStartValue, 0) : 0;
    var target = 0;
    var balance = actual - target;
    var resetInterval = settings.flexiEnabled ? (settings.resetInterval || "NONE") : "NONE";
    var nextReset = resetInterval !== "NONE" ? nextResetAfter(startDay - 1, resetInterval) : -1;

    var carry = null; // last event of previous day (clock-in carry)
    var workedToday = 0;
    var todayTarget = 0;

    // Bucket events by local day once (O(events)); the per-day scan was
    // O(days × events). The offset at each event's own instant keeps DST
    // transition days correct.
    var eventsByDay = bucketEventsByDay(evs, offFn);
    var dayStartCache = {};

    for (var day = startDay; day <= todayEpoch; day++) {
        var dayEvents = eventsByDay[day] || [];
        var dayStartMs = dayStartFor(day, offFn, dayStartCache);
        var dayEndMs = dayStartFor(day + 1, offFn, dayStartCache);
        var isToday = day === todayEpoch;

        var effective = effectiveEventsForDay(dayEvents, carry, isToday, nowMs, futureClockOutExists);
        var dayRes = workDay(day, effective, carry, nowMs, dayStartMs, dayEndMs, isToday, dayTargets, normalWork, settings.workdays);

        // flexi reset: cumulative target = actual on reset date
        if (resetInterval !== "NONE" && nextReset === day) {
            target = actual;
            nextReset = nextResetAfter(day, resetInterval);
        }

        actual += dayRes.worked;
        target += dayRes.target;

        // The app keeps the displayed balance unchanged on DAY_IGNORE.
        var special = dayTargets[epochDayToISODate(day)] || null;
        if (!special || special.type !== "DAY_IGNORE") {
            balance = actual - target;
        }

        if (isToday) {
            workedToday = dayRes.worked;
            todayTarget = dayRes.target;
        }
        carry = dayRes.lastEvent;
    }

    // pill clock-in state: last event at or before now
    var lastSeen = null;
    for (var m = 0; m < evs.length; m++) {
        if (evs[m].time * 1000 <= nowMs) {
            lastSeen = evs[m];
        } else {
            break;
        }
    }

    result.workedToday = workedToday;
    result.todayTarget = todayTarget;
    result.balance = settings.flexiEnabled ? balance : 0;
    result.clockedIn = lastSeen !== null && lastSeen.type === "CLOCK_IN";
    result.lastEventTime = lastSeen ? lastSeen.time : 0;
    return result;
}

// Per-day rows for the calendar week starting at `weekStartDay` (an epoch
// day, local). Mirrors the app's WeekStateCalculator: in/out/worked/target,
// and the cumulative flexi balance at the end of each day.
export function calculateWeek(events, targets, settings, nowMs, homeOffset, weekStartDay) {
    var offFn = resolveOffset(homeOffset);
    var todayEpoch = todayEpochDay(nowMs, offFn(nowMs));
    var weekEnd = weekStartDay + 6;
    var evs = normalizeEvents(events);
    var dayTargets = buildDayTargets(targets);
    var normalWork = normalWorkMinutes(settings);
    var futureClockOutExists = hasFutureClockOut(evs, nowMs);
    var eventsByDay = bucketEventsByDay(evs, offFn);
    var dayStartCache = {};

    var flexiEnabled = !!settings.flexiEnabled;
    var resetInterval = flexiEnabled ? (settings.resetInterval || "NONE") : "NONE";
    var actual = flexiEnabled ? parseHoursMinutes(settings.flexiStartValue, 0) : 0;
    var target = 0;
    var balance = actual - target;
    var nextReset = -1;
    var carry = null;
    var computed = false;
    var rows = new Map();

    if (weekStartDay <= todayEpoch) {
        var firstDay = evs.length
            ? todayEpochDay(evs[0].time * 1000, offFn(evs[0].time * 1000))
            : weekStartDay;
        var startDay = Math.min(mondayOf(firstDay), weekStartDay);
        var lastDay = Math.min(weekEnd, todayEpoch);
        if (resetInterval !== "NONE") {
            nextReset = nextResetAfter(startDay - 1, resetInterval);
        }

        for (var day = startDay; day <= lastDay; day++) {
            var dayEvents = eventsByDay[day] || [];
            var dayStartMs = dayStartFor(day, offFn, dayStartCache);
            var dayEndMs = dayStartFor(day + 1, offFn, dayStartCache);
            var isToday = day === todayEpoch;
            var effective = effectiveEventsForDay(dayEvents, carry, isToday, nowMs, futureClockOutExists);

            if (day === weekStartDay) {
                computed = true;
            }

            var dayRes = workDay(day, effective, carry, nowMs, dayStartMs, dayEndMs, isToday, dayTargets, normalWork, settings.workdays);

            if (resetInterval !== "NONE" && nextReset === day) {
                target = actual;
                nextReset = nextResetAfter(day, resetInterval);
            }

            actual += dayRes.worked;
            target += dayRes.target;

            var dateISO = epochDayToISODate(day);
            var special = dayTargets[dateISO] || null;
            // The app keeps the displayed balance unchanged on DAY_IGNORE.
            if (!special || special.type !== "DAY_IGNORE") {
                balance = actual - target;
            }

            if (day >= weekStartDay) {
                rows.set(day, {
                    epochDay: day,
                    date: dateISO,
                    inMs: dayRes.inMs,
                    outMs: dayRes.outMs,
                    worked: dayRes.worked,
                    target: dayRes.target,
                    balance: balance,
                    isToday: isToday,
                    future: false,
                    hasEvents: dayEvents.length > 0
                });
            }
            carry = dayRes.lastEvent;
        }
    }

    // Every day of the week gets a row; days after today stay blank.
    for (var d = weekStartDay; d <= weekEnd; d++) {
        if (!rows.has(d)) {
            rows.set(d, {
                epochDay: d,
                date: epochDayToISODate(d),
                inMs: null,
                outMs: null,
                worked: null,
                target: null,
                balance: null,
                isToday: d === todayEpoch,
                future: d > todayEpoch,
                hasEvents: false
            });
        }
    }

    var days = [];
    var totalWorked = 0;
    var totalTarget = 0;
    for (var e = weekStartDay; e <= weekEnd; e++) {
        var row = rows.get(e);
        days.push(row);
        if (row.worked !== null) totalWorked += row.worked;
        if (row.target !== null) totalTarget += row.target;
    }

    return {
        days: days,
        weekNumber: isoWeekNumber(weekStartDay),
        totals: {
            // Row sums stay truthful across a flexi reset on the week's first
            // day (the reset rewrites the cumulative `target`). A week that has
            // not started yet has no totals at all (the table shows dashes).
            worked: computed ? totalWorked : null,
            target: computed ? totalTarget : null,
            balance: computed && flexiEnabled ? balance : null
        }
    };
}

// ---------- day-level calculation (mirrors calculateNextDay + calculateWorkTime) ----------

function workDay(day, dayEvents, carry, nowMs, dayStartMs, dayEndMs, isToday, dayTargets, normalWork, workdays) {
    var dateISO = epochDayToISODate(day);

    // --- calculateWorkTime port ---
    var clockedInSince = null; // ms epoch
    var worked = 0;
    var firstClockInMs = null;
    var effectiveClockOutMs = null;
    var inMs = null;
    var outMs = null;

    // carry: clocked in since start of day
    if (carry !== null && carry.type === "CLOCK_IN" && dayEvents.length > 0 && dayEvents[0].type !== "CLOCK_IN") {
        clockedInSince = dayStartMs;
        inMs = dayStartMs;
    }

    var k;
    for (k = 0; k < dayEvents.length; k++) {
        var ev = dayEvents[k];
        var evMs = ev.time * 1000;
        if (ev.type === "CLOCK_IN" && clockedInSince === null) {
            clockedInSince = evMs;
            if (firstClockInMs === null) {
                firstClockInMs = evMs;
            }
        } else if (clockedInSince !== null && ev.type === "CLOCK_OUT") {
            worked += diffMinutes(clockedInSince, evMs);
            clockedInSince = null;
            effectiveClockOutMs = evMs;
        }
    }

    // still clocked in at end of day (and now is after the clock-in instant)
    if (clockedInSince !== null && nowMs > clockedInSince) {
        var dayEndMinute = dayEndMs - 60000; // 23:59 local
        outMs = dayEndMinute;
        worked += diffMinutes(clockedInSince, dayEndMinute);
    } else if (effectiveClockOutMs !== null) {
        outMs = effectiveClockOutMs;
    }

    if (firstClockInMs !== null && inMs === null) {
        inMs = firstClockInMs;
    }

    // no events this day but clocked in all day (mirrors calculateWorkTime else-branch)
    if (dayEvents.length === 0 && carry !== null && carry.type === "CLOCK_IN" && nowMs > carry.time * 1000) {
        inMs = dayStartMs;
        outMs = dayEndMs - 60000;
        worked = 24 * 60;
    }

    var lastEvent = dayEvents.length > 0 ? dayEvents[dayEvents.length - 1] : carry;

    // --- target for this day (mirrors calculateTargetTime) ---
    var special = dayTargets[dateISO] || null;
    var targetEnum = special ? special.type : null;
    var dayTarget = targetForDay(dateISO, day, dayTargets, normalWork, workdays);

    // DAY_GRANT bump for past days: worked = max(worked, dayTarget)
    if (!isToday && targetEnum === "DAY_GRANT" && dayTarget !== 0 && worked < dayTarget) {
        worked = dayTarget;
    }

    return {
        worked: worked,
        target: dayTarget,
        inMs: inMs,
        outMs: outMs,
        lastEvent: lastEvent
    };
}

// ---------- helpers shared with callers ----------

function targetForDay(dateISO, day, dayTargets, normalWork, workdays) {
    var special = dayTargets[dateISO] || null;
    var targetEnum = special ? special.type : null;
    var weekdayIdx = ((day + 3) % 7 + 7) % 7; // 0 = Monday (epoch day 0 = Thursday)
    var isRegularWorkday = workdays[weekdayIdx];

    if (targetEnum === "DAY_SET") {
        // value 0 = free day override; DAY_GRANT / DAY_IGNORE keep the
        // regular target (0 on free days)
        return special.value === 0 || special.value === null || special.value === undefined
            ? 0
            : special.value;
    }
    return isRegularWorkday ? normalWork : 0;
}

export function normalizeEvents(events) {
    var evs = [];
    for (var i = 0; i < (events || []).length; i++) {
        var e = events[i];
        if (e.deletedAt !== undefined && e.deletedAt !== null) {
            continue;
        }
        if (e.type !== "CLOCK_IN" && e.type !== "CLOCK_OUT") {
            continue;
        }
        evs.push({ time: e.time, type: e.type, order: evs.length });
    }
    // The API breaks same-time ties by insertion order. Keep that order while
    // sorting by time (Array.sort is stable in modern engines, but be explicit).
    evs.sort(function (a, b) { return (a.time - b.time) || (a.order - b.order); });
    return evs;
}

// Mirrors DAO.getDayTarget for conflicting rows: a date with differing day
// targets yields no special target, so it must not resolve by API order.
// Rows that are identical for calculation (same type and value, e.g. a
// holiday duplicated by an import and a later sync) collapse to one row:
// they express the same decision, and the app's "duplicate => none" rule
// would otherwise let a stale copy silently turn a free day into a workday.
function buildDayTargets(targets) {
    var dayTargets = {};
    var seen = {};
    var conflicted = {};
    for (var i = 0; i < (targets || []).length; i++) {
        var t = targets[i];
        if (t.deletedAt) {
            continue;
        }
        if (t.type === "FLEXI_SET" || t.type === "FLEXI_ADD") {
            continue;
        }
        if (conflicted[t.date]) {
            continue;
        }
        var first = seen[t.date];
        if (first === undefined) {
            seen[t.date] = t;
            dayTargets[t.date] = t;
        } else if (first.type !== t.type || first.value !== t.value) {
            conflicted[t.date] = true;
            dayTargets[t.date] = null;
        }
    }
    return dayTargets;
}

function normalWorkMinutes(settings) {
    var countWorkDays = 0;
    for (var i = 0; i < 7; i++) {
        if (settings.workdays[i]) {
            countWorkDays++;
        }
    }
    return countWorkDays > 0
        ? roundHalfUpDivide(parseHoursMinutes(settings.flexiTarget, 0), countWorkDays)
        : 0;
}

function hasFutureClockOut(evs, nowMs) {
    for (var i = 0; i < evs.length; i++) {
        if (evs[i].type === "CLOCK_OUT" && evs[i].time * 1000 > nowMs) {
            return true;
        }
    }
    return false;
}

function effectiveEventsForDay(dayEvents, carry, isToday, nowMs, futureClockOutExists) {
    if (!isToday) {
        return dayEvents;
    }
    var effective = [];
    var lastUpToNow = carry;
    var j;
    for (j = 0; j < dayEvents.length; j++) {
        if (dayEvents[j].time * 1000 <= nowMs) {
            effective.push(dayEvents[j]);
            lastUpToNow = dayEvents[j];
        }
    }
    // Java: synthetic CLOCK_OUT at "now" when the day ends clocked-in
    // (also when only a carry-over clock-in exists and today has no events yet)
    // and no future CLOCK_OUT event exists.
    var clockedInAsOfNow = (lastUpToNow !== null && lastUpToNow.type === "CLOCK_IN")
        || (lastUpToNow === null && carry !== null && carry.type === "CLOCK_IN");
    if (clockedInAsOfNow && !futureClockOutExists) {
        effective.push({ time: Math.floor(nowMs / 1000), type: "CLOCK_OUT", synthetic: true });
    }
    for (var k = 0; k < dayEvents.length; k++) {
        if (dayEvents[k].time * 1000 > nowMs) {
            effective.push(dayEvents[k]);
        }
    }
    return effective;
}

function resolveOffset(homeOffset) {
    return typeof homeOffset === "function"
        ? homeOffset
        : function () { return homeOffset; };
}

function bucketEventsByDay(evs, offFn) {
    var byDay = {};
    for (var i = 0; i < evs.length; i++) {
        var evMs = evs[i].time * 1000;
        var evDay = todayEpochDay(evMs, offFn(evMs));
        if (byDay[evDay] === undefined) {
            byDay[evDay] = [];
        }
        byDay[evDay].push(evs[i]);
    }
    return byDay;
}

// First minute of local day `day` in the home offset. Most days resolve from
// the offset at UTC midnight; transition days that do not fall back to a
// minute-resolution search. Cached because the day loop asks for D and D+1.
function dayStartFor(day, offFn, cache) {
    var cached = cache[day];
    if (cached !== undefined) {
        return cached;
    }
    var start = localDayStartMs(day, offFn);
    cache[day] = start;
    return start;
}

function localDayStartMs(day, offFn) {
    var t = day * 86400000;
    var guess = t - offFn(t) * 60000;
    if (todayEpochDay(guess, offFn(guess)) === day
            && todayEpochDay(guess - 60000, offFn(guess - 60000)) < day) {
        return guess;
    }

    // Zone transition at/near local midnight: binary-search the first UTC
    // minute whose local date is `day` (local dates never move backwards
    // across a calendar boundary, even at fall-back).
    var lo = t - 36 * 3600000;
    var hi = t + 36 * 3600000;
    while (hi - lo > 60000) {
        var mid = lo + Math.floor((hi - lo) / 120000) * 60000;
        if (mid <= lo) {
            mid = lo + 60000;
        }
        if (todayEpochDay(mid, offFn(mid)) < day) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return hi;
}

export function epochDayToISODate(epochDay) {
    return new Date(epochDay * 86400000).toISOString().slice(0, 10);
}

export function todayEpochDay(nowMs, tzOffsetMinutes) {
    return Math.floor((nowMs + tzOffsetMinutes * 60000) / 86400000);
}

export function mondayOf(day) {
    // epoch day 0 (1970-01-01) was a Thursday → (d+3)%7 = 0 on Mondays
    return day - ((day + 3) % 7);
}

function diffMinutes(fromMs, toMs) {
    // TimerManager.timeDiff truncates both operands to minutes
    return Math.floor(toMs / 60000) - Math.floor(fromMs / 60000);
}

function roundHalfUpDivide(total, divisor) {
    // BigDecimal.divide(..., HALF_UP).intValue()
    return Math.round(total / divisor);
}

export function parseHoursMinutes(str, fallback) {
    // TimerManager.parseHoursMinutesString
    if (str === null || str === undefined || str === "") {
        return fallback;
    }
    var trimmed = String(str).trim();
    var negative = trimmed.indexOf("-") === 0;
    var s = trimmed.replace(/[- ]/g, "");
    var parts = s.split(/[:.]/);
    var h = parseInt(parts[0], 10);
    var m = parts.length > 1 ? parseInt(parts[1], 10) : 0;
    if (isNaN(h)) h = 0;
    if (isNaN(m)) m = 0;
    var total = h * 60 + m;
    return negative ? -total : total;
}

export function formatMinutes(minutes) {
    // TimerManager.formatTime: "H:MM" / "-H:MM"
    var sign = minutes < 0 ? "-" : "";
    var abs = Math.abs(Math.round(minutes));
    var h = Math.floor(abs / 60);
    var m = abs % 60;
    return sign + h + ":" + (m < 10 ? "0" + m : "" + m);
}

// ---------- flexi reset scheduling (FlexiReset.getNextResetDate port) ----------

var RESET_SIZES = { DAILY: 1, WEEKLY: 1, MONTHLY: 1, QUARTERLY: 3, HALF_YEARLY: 6, YEARLY: 12 };

function nextResetAfter(day, interval) {
    var size = RESET_SIZES[interval];
    if (size === undefined) {
        return -1;
    }
    if (interval === "DAILY") {
        return day + size;
    }
    if (interval === "WEEKLY") {
        return mondayOf(day) + 7 * size;
    }
    // month-based intervals
    var ymd = dayToYMD(day);
    var monthsDelta = (ymd.m - 1) % size;
    var anchorMonth = ymd.y * 12 + (ymd.m - 1) - monthsDelta; // day 1 of anchor month
    var nextMonth = anchorMonth + size;
    return ymdToEpochDay(Math.floor(nextMonth / 12), (nextMonth % 12) + 1, 1);
}

function dayToYMD(epochDay) {
    var d = new Date(epochDay * 86400000);
    return { y: d.getUTCFullYear(), m: d.getUTCMonth() + 1, d: d.getUTCDate() };
}

function ymdToEpochDay(y, m, d) {
    return Math.floor(Date.UTC(y, m - 1, d) / 86400000);
}

export function isoWeekNumber(epochDay) {
    // ISO-8601 week number of the date's Thursday.
    var d = new Date(epochDay * 86400000);
    var target = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()));
    var dayNr = (target.getUTCDay() + 6) % 7;
    target.setUTCDate(target.getUTCDate() - dayNr + 3);
    var firstThursday = new Date(Date.UTC(target.getUTCFullYear(), 0, 4));
    var firstDayNr = (firstThursday.getUTCDay() + 6) % 7;
    firstThursday.setUTCDate(firstThursday.getUTCDate() - firstDayNr + 3);
    return 1 + Math.round((target.getTime() - firstThursday.getTime()) / (7 * 86400000));
}
