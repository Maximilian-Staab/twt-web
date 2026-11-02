// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Events: the app's event list with add/edit/delete. Same payload shape as
// the app (epoch seconds, zone offset in minutes, CLOCK_IN/CLOCK_OUT only).

import { escapeHtml, openDialog, confirmDialog } from "../ui.js";
import {
    formatClock,
    formatDateLabel,
    inputDateTimeToEpoch,
    localDateISO,
    localTimeHHMM,
} from "../format.js";
import {
    emptyState,
    eventRowHtml,
    taskOptionsHtml,
} from "./common.js";

const RANGES = [
    ["7", "Last 7 days"],
    ["30", "Last 30 days"],
    ["90", "Last 90 days"],
    ["365", "Last year"],
    ["all", "All events"],
];

export function renderEvents(root, ctx) {
    const {state} = ctx;
    const events = filteredEvents(state);
    const groups = groupByDay(events);

    root.innerHTML = `
        <section class="view-head">
            <div>
                <h1>Events</h1>
                <p class="muted">Clock-in and clock-out records, newest first.</p>
            </div>
            <div class="toolbar">
                <label class="inline-label">Range
                    <select data-range>
                        ${RANGES.map(([value, label]) =>
                            `<option value="${value}"${String(state.eventRangeDays) === value ? " selected" : ""}>${label}</option>`,
                        ).join("")}
                    </select>
                </label>
                <button class="btn primary" data-action="add-event">Add event</button>
            </div>
        </section>

        <section class="card">
            ${events.length ? `
                <div class="table-wrap">
                    <table class="data-table events-table">
                        <thead>
                            <tr>
                                <th scope="col">Time</th>
                                <th scope="col">Type</th>
                                <th scope="col">Task</th>
                                <th scope="col">Comment</th>
                                <th scope="col"><span class="sr-only">Actions</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            ${[...groups.entries()].map(([date, rows]) => `
                                <tr class="day-separator">
                                    <th colspan="5">
                                        ${escapeHtml(formatDateLabel(new Date(`${date}T00:00:00`).getTime()))}
                                        <span class="muted">· ${rows.length} event${rows.length === 1 ? "" : "s"}</span>
                                    </th>
                                </tr>
                                ${rows.map((event) => eventRowHtml(state, event)).join("")}
                            `).join("")}
                        </tbody>
                    </table>
                </div>
            ` : emptyState("No events in this range.")}
        </section>
    `;

    root.querySelector("[data-range]")?.addEventListener("change", (event) => {
        const value = event.target.value;
        state.eventRangeDays = value === "all" ? "all" : Number(value);
        renderEvents(root, ctx);
    });

    root.querySelector('[data-action="add-event"]')?.addEventListener("click", () => {
        openEventDialog(ctx, null);
    });

    root.querySelector("tbody")?.addEventListener("click", async (event) => {
        const button = event.target.closest("button[data-action]");
        if (!button) return;
        const record = state.events.find((e) => e.id === button.dataset.id);
        if (!record) return;

        if (button.dataset.action === "edit-event") {
            openEventDialog(ctx, record);
            return;
        }
        if (button.dataset.action === "delete-event") {
            const ok = await confirmDialog(
                `Delete this ${record.type === "CLOCK_IN" ? "clock-in" : "clock-out"} ` +
                `at ${formatClock(record.time * 1000)}?`,
                {title: "Delete event", confirmLabel: "Delete"},
            );
            if (!ok) return;
            button.disabled = true;
            try {
                await ctx.mutate(
                    () => ctx.api.deleteEvent(record.id, record.updatedAt),
                    "Event deleted.",
                );
            } catch {
                button.disabled = false;
            }
        }
    });
}

function filteredEvents(state) {
    const range = state.eventRangeDays;
    const cutoff = range === "all"
        ? -Infinity
        : Date.now() - Number(range) * 86400000;
    return state.events
        .filter((event) => !event.deletedAt)
        .filter((event) => event.time * 1000 >= cutoff)
        .sort((a, b) => b.time - a.time);
}

function groupByDay(events) {
    const groups = new Map();
    for (const event of events) {
        const key = localDateISO(event.time * 1000);
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(event);
    }
    return groups;
}

function openEventDialog(ctx, event) {
    const {state} = ctx;
    const isEdit = Boolean(event);
    const now = Date.now();
    const at = isEdit ? event.time * 1000 : now;
    const type = isEdit
        ? event.type
        : (state.calc && state.calc.clockedIn ? "CLOCK_OUT" : "CLOCK_IN");
    const taskId = isEdit ? (event.task || "") : (state.clock.taskId || "");

    openDialog({
        title: isEdit ? "Edit event" : "Add event",
        submitLabel: isEdit ? "Save event" : "Create event",
        bodyHtml: `
            <div class="form-grid">
                <label>Date
                    <input type="date" name="date" required value="${escapeHtml(localDateISO(at))}">
                </label>
                <label>Time
                    <input type="time" name="time" required value="${escapeHtml(localTimeHHMM(at))}">
                </label>
                <label>Type
                    <select name="type">
                        <option value="CLOCK_IN"${type === "CLOCK_IN" ? " selected" : ""}>Clock in</option>
                        <option value="CLOCK_OUT"${type === "CLOCK_OUT" ? " selected" : ""}>Clock out</option>
                    </select>
                </label>
                <label>Task
                    <select name="task">
                        <!-- The backend PATCH treats task:null as "unchanged",
                             so clearing an attached task is not offered. -->
                        ${taskOptionsHtml(state, taskId, {
                            includeAll: true,
                            includeNone: !isEdit || !event.task,
                        })}
                    </select>
                </label>
            </div>
            <label>Comment
                <input type="text" name="comment" maxlength="500"
                       value="${escapeHtml(event && event.comment ? event.comment : "")}"
                       placeholder="optional">
            </label>
        `,
        onSubmit: async (formData) => {
            const date = formData.get("date");
            const time = formData.get("time");
            if (!date || !time) {
                throw new Error("Date and time are required.");
            }
            const {time: epochSeconds, zoneOffset} = inputDateTimeToEpoch(date, time);
            const payload = {
                time: epochSeconds,
                zoneOffset,
                type: String(formData.get("type")),
                task: String(formData.get("task") || "") || null,
                comment: String(formData.get("comment") || "").trim(),
            };
            if (isEdit) {
                payload.updatedAt = event.updatedAt;
            }
            await ctx.mutate(
                () => isEdit
                    ? ctx.api.updateEvent(event.id, payload)
                    : ctx.api.createEvent(payload),
                isEdit ? "Event updated." : "Event added.",
            );
        },
    });
}
