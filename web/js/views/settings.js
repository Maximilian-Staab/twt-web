// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Settings: the twtBar plugin's flexi configuration (browser-local) plus the
// signed-in account and server URL.

import { escapeHtml } from "../ui.js";
import { parseDurationInput } from "../format.js";
import { normalizeApiBase } from "../auth.js";
import {
    RESET_INTERVALS,
    maskFromWorkdays,
    saveSettings,
    workdaysFromMask,
} from "../settings.js";

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export function renderSettings(root, ctx) {
    const {state, auth} = ctx;
    const settings = state.settings;
    const workdays = workdaysFromMask(settings.workdaysMask);

    root.innerHTML = `
        <section class="view-head">
            <div>
                <h1>Settings</h1>
                <p class="muted">Calculation settings and the signed-in account.</p>
            </div>
        </section>

        <div class="settings-grid">
            <section class="card">
                <h2>Flexi time</h2>
                <p class="muted small">
                    These values drive the dashboard and week calculations in this
                    browser. The app and the twtBar plugin keep their own settings.
                </p>
                <form id="settings-form" novalidate>
                    <label class="check">
                        <input type="checkbox" name="flexiEnabled"${settings.flexiEnabled ? " checked" : ""}>
                        Enable flexi time
                    </label>

                    <div class="form-grid">
                        <label>Starting balance
                            <input type="text" name="flexiStartValue" inputmode="numeric"
                                   value="${escapeHtml(settings.flexiStartValue)}" placeholder="0:00">
                            <span class="hint">Credit at the start of tracking, H:MM.</span>
                        </label>
                        <label>Weekly target
                            <input type="text" name="flexiTarget" inputmode="numeric"
                                   value="${escapeHtml(settings.flexiTarget)}" placeholder="40:00">
                            <span class="hint">Target work per week, H:MM.</span>
                        </label>
                        <label>Reset interval
                            <select name="resetInterval">
                                ${RESET_INTERVALS.map(([value, label]) =>
                                    `<option value="${value}"${value === settings.resetInterval ? " selected" : ""}>${label}</option>`,
                                ).join("")}
                            </select>
                        </label>
                    </div>

                    <fieldset class="workdays">
                        <legend>Workdays</legend>
                        ${WEEKDAYS.map((label, index) => `
                            <label class="check">
                                <input type="checkbox" name="workday" value="${index}"
                                       ${workdays[index] ? "checked" : ""}>
                                ${label}
                            </label>
                        `).join("")}
                    </fieldset>

                    <p class="form-error" data-settings-error hidden></p>
                    <button type="submit" class="btn primary">Save settings</button>
                </form>
            </section>

            <section class="card">
                <h2>Account</h2>
                <dl class="facts">
                    <dt>Subject</dt>
                    <dd>${escapeHtml(auth.subject || "—")}</dd>
                    <dt>Issuer</dt>
                    <dd class="wrap">${escapeHtml(auth.issuer || "—")}</dd>
                    <dt>API base</dt>
                    <dd class="wrap">${escapeHtml(state.apiBase)}</dd>
                </dl>
                <button class="btn ghost" data-action="signout">Sign out</button>

                <hr>

                <h3>Change server</h3>
                <label>API base URL
                    <input type="url" data-server-input value="${escapeHtml(state.apiBase)}"
                           placeholder="http://localhost:8080/api/v1">
                    <span class="hint">Server root or a URL ending in /api/v1.</span>
                </label>
                <button class="btn ghost" data-action="change-server">Change server &amp; sign in again</button>
            </section>
        </div>
    `;

    const form = root.querySelector("#settings-form");
    const errorEl = root.querySelector("[data-settings-error]");
    form.addEventListener("submit", (event) => {
        event.preventDefault();
        errorEl.hidden = true;
        try {
            const formData = new FormData(form);
            const next = {
                flexiEnabled: formData.get("flexiEnabled") === "on",
                flexiStartValue: String(formData.get("flexiStartValue") || "").trim(),
                flexiTarget: String(formData.get("flexiTarget") || "").trim(),
                resetInterval: String(formData.get("resetInterval")),
                workdaysMask: maskFromWorkdays(
                    WEEKDAYS.map((_, index) => formData.getAll("workday").includes(String(index))),
                ),
            };
            parseDurationInput(next.flexiStartValue);
            parseDurationInput(next.flexiTarget);
            state.settings = next;
            saveSettings(next);
            ctx.recompute();
            ctx.toast("Settings saved.", "ok");
        } catch (error) {
            errorEl.textContent = error.message || String(error);
            errorEl.hidden = false;
        }
    });

    root.querySelector('[data-action="signout"]')?.addEventListener("click", () => {
        ctx.signOut();
    });

    root.querySelector('[data-action="change-server"]')?.addEventListener("click", () => {
        const input = root.querySelector("[data-server-input]");
        try {
            const next = normalizeApiBase(input.value);
            if (next === state.apiBase) {
                ctx.toast("That is already the active server.", "info");
                return;
            }
            ctx.changeServer(next);
        } catch (error) {
            ctx.toast(error.message || String(error), "error");
        }
    });
}
