// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Helpers shared by the feature views.

import { escapeHtml } from "../ui.js";
import { formatClock, formatMinutes } from "../format.js";

export const EVENT_TYPE_LABEL = {
    CLOCK_IN: "Clock in",
    CLOCK_OUT: "Clock out",
};

export const TARGET_TYPE_LABEL = {
    DAY_SET: "Day target",
    DAY_GRANT: "Day grant",
    DAY_IGNORE: "Day ignore",
    FLEXI_SET: "Flexi set",
    FLEXI_ADD: "Flexi add",
};

export function taskName(state, id) {
    if (!id) return "";
    const task = state.tasks.find((t) => t.id === id);
    return task ? task.name : "deleted task";
}

export function activeTasks(state) {
    return state.tasks
        .filter((t) => t.active)
        .sort((a, b) => a.name.localeCompare(b.name));
}

export function defaultTaskId(state) {
    const active = activeTasks(state);
    const preferred = active.find((t) => t.isDefault);
    // null = nothing to preselect; the dashboard treats "" as an explicit
    // "no task" choice and leaves it alone across refreshes.
    return preferred ? preferred.id : (active[0] ? active[0].id : null);
}

export function taskOptionsHtml(state, selectedId, {includeNone = true, includeAll = false} = {}) {
    const tasks = includeAll ? state.tasks : activeTasks(state);
    const known = new Set(tasks.map((t) => t.id));
    const options = [];
    if (includeNone) {
        options.push(`<option value="">— no task —</option>`);
    }
    for (const task of tasks) {
        const selected = task.id === selectedId ? " selected" : "";
        options.push(
            `<option value="${escapeHtml(task.id)}"${selected}>${escapeHtml(task.name)}` +
            `${task.active ? "" : " (inactive)"}</option>`,
        );
    }
    if (selectedId && !known.has(selectedId)) {
        options.push(`<option value="${escapeHtml(selectedId)}" selected>deleted task</option>`);
    }
    return options.join("");
}

export function eventTypeBadge(type) {
    const label = EVENT_TYPE_LABEL[type] || type;
    const cls = type === "CLOCK_IN" ? "badge-in" : "badge-out";
    return `<span class="badge ${cls}">${escapeHtml(label)}</span>`;
}

export function eventRowHtml(state, event, {withActions = true} = {}) {
    const task = taskName(state, event.task);
    const comment = event.comment ? escapeHtml(event.comment) : "";
    return `
        <tr>
            <td class="mono">${escapeHtml(formatClock(event.time * 1000))}</td>
            <td>${eventTypeBadge(event.type)}</td>
            <td>${task ? escapeHtml(task) : '<span class="muted">—</span>'}</td>
            <td class="comment-cell">${comment || '<span class="muted">—</span>'}</td>
            ${withActions ? `
            <td class="actions">
                <button class="btn small ghost" data-action="edit-event" data-id="${escapeHtml(event.id)}">Edit</button>
                <button class="btn small ghost danger-text" data-action="delete-event" data-id="${escapeHtml(event.id)}">Delete</button>
            </td>` : ""}
        </tr>
    `;
}

export function eventsOnLocalDay(state, epochDay) {
    return state.events
        .filter((event) => !event.deletedAt)
        .filter((event) => {
            const d = new Date(event.time * 1000);
            const localDay = Math.floor(
                Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / 86400000,
            );
            return localDay === epochDay;
        })
        .sort((a, b) => a.time - b.time);
}

export function formatTargetValue(minutes) {
    return minutes === null || minutes === undefined
        ? '<span class="muted">—</span>'
        : escapeHtml(formatMinutes(minutes));
}

export function emptyState(message) {
    return `<p class="empty-state">${escapeHtml(message)}</p>`;
}
