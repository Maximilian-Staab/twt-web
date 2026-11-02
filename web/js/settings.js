// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Browser-local settings. Mirrors the twtBar plugin's settings (flexi time,
// workdays, reset interval); they never leave this browser.

export const API_BASE_KEY = "twt.apiBase.v1";
export const SETTINGS_KEY = "twt.settings.v1";
export const DEFAULT_API_BASE = "http://localhost:8080/api/v1";

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
