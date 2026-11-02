// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Browser-local settings. Mirrors the twtBar plugin's settings (flexi time,
// workdays, reset interval); they never leave this browser.

export const API_BASE_KEY = "twt.apiBase.v1";
export const SETTINGS_KEY = "twt.settings.v1";

// The documented dev server (web/README.md) serves the page on :5173 while the
// API listens on the devenv/compose port 8080, so same-origin would be wrong
// there. Every other deployment is same-origin behind the proxy.
export const DEV_API_BASE = "http://localhost:8080/api/v1";
const DEV_SERVER_PORT = "5173";

/**
 * Default API base for a first visit: `<page origin>/api/v1`.
 *
 * The production layout (compose proxy, Pangolin, any reverse proxy) serves
 * the frontend and `/api/v1` from one origin, so the page origin is the right
 * guess and the login form needs no input. The dev server on :5173 is the one
 * exception: it has no API of its own.
 */
export function defaultApiBase(loc = globalThis.location) {
    if (!loc || typeof loc.origin !== "string" || loc.origin === "null") {
        return DEV_API_BASE;
    }
    return loc.port === DEV_SERVER_PORT ? DEV_API_BASE : `${loc.origin}/api/v1`;
}

export const RESET_INTERVALS = [
    ["NONE", "Never"],
    ["DAILY", "Daily"],
    ["WEEKLY", "Weekly"],
    ["MONTHLY", "Monthly"],
    ["QUARTERLY", "Quarterly"],
    ["HALF_YEARLY", "Half-yearly"],
    ["YEARLY", "Yearly"],
];

export const DEFAULT_SETTINGS = {
    flexiEnabled: true,
    flexiStartValue: "0:00",
    flexiTarget: "40:00",
    resetInterval: "NONE",
    workdaysMask: 31, // bit 0 = Monday
};

export function loadSettings() {
    try {
        const raw = localStorage.getItem(SETTINGS_KEY);
        if (!raw) return {...DEFAULT_SETTINGS};
        const parsed = JSON.parse(raw);
        return {
            flexiEnabled: parsed.flexiEnabled !== false,
            flexiStartValue: typeof parsed.flexiStartValue === "string"
                ? parsed.flexiStartValue
                : DEFAULT_SETTINGS.flexiStartValue,
            flexiTarget: typeof parsed.flexiTarget === "string"
                ? parsed.flexiTarget
                : DEFAULT_SETTINGS.flexiTarget,
            resetInterval: RESET_INTERVALS.some(([value]) => value === parsed.resetInterval)
                ? parsed.resetInterval
                : DEFAULT_SETTINGS.resetInterval,
            workdaysMask: Number.isInteger(parsed.workdaysMask)
                ? parsed.workdaysMask & 0x7f
                : DEFAULT_SETTINGS.workdaysMask,
        };
    } catch {
        return {...DEFAULT_SETTINGS};
    }
}

export function saveSettings(settings) {
    try {
        localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    } catch {
        // private mode / quota: keep the in-memory settings
    }
}

export function workdaysFromMask(mask) {
    const days = [];
    for (let i = 0; i < 7; i++) {
        days.push(((mask >> i) & 1) === 1);
    }
    return days;
}

export function maskFromWorkdays(selected) {
    let mask = 0;
    for (let i = 0; i < 7; i++) {
        if (selected[i]) mask |= 1 << i;
    }
    return mask;
}

export function engineSettings(settings) {
    return {
        flexiEnabled: settings.flexiEnabled,
        flexiStartValue: settings.flexiStartValue,
        flexiTarget: settings.flexiTarget,
        resetInterval: settings.resetInterval,
        workdays: workdaysFromMask(settings.workdaysMask),
    };
}
