// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Dashboard: the twtBar pill as a page — worked today, flexi balance,
// clock in/out, the app's week table, and today's events.

import { escapeHtml, setBusy } from "../ui.js";
import {
    formatClock,
    formatEpochDayShort,
    formatMinutes,
    formatWeekday,
    localEpochDay,
    mondayOf,
} from "../format.js";
import {
    activeTasks,
    defaultTaskId,
    emptyState,
    eventsOnLocalDay,
    taskOptionsHtml,
    taskName,
    eventTypeBadge,
} from "./common.js";

export function renderDashboard(root, ctx) {
    const {state} = ctx;
    // Keep the selected task valid after a refresh changes the task list.
    // "" is an explicit "no task" choice and must survive refreshes.
    if (
        state.clock.taskId === null ||
        (state.clock.taskId !== "" && !activeTasks(state).some((t) => t.id === state.clock.taskId))
    ) {
        state.clock.taskId = defaultTaskId(state);
    }
    const calc = state.calc || {
        workedToday: 0,
        todayTarget: 0,
        balance: 0,
        clockedIn: false,
        lastEventTime: 0,
        flexiUsed: state.settings.flexiEnabled,
    };
    const week = state.week;
    const today = localEpochDay(Date.now());
    const todayEvents = eventsOnLocalDay(state, today);
    const balanceClass = calc.balance < 0 ? "negative" : calc.balance > 0 ? "positive" : "";

    root.innerHTML = `
        <section class="view-head">
            <div>
                <h1>Dashboard</h1>
                <p class="muted">Worked today, flexi balance and clock in/out.</p>
            </div>
            <button class="btn ghost" data-action="refresh">Refresh</button>
        </section>

        <div class="dashboard-grid">
            <section class="card status-card ${calc.clockedIn ? "is-in" : "is-out"}">
                <div class="status-top">
                    <span class="status-pill ${calc.clockedIn ? "in" : "out"}">
                        ${calc.clockedIn ? "Clocked in" : "Clocked out"}
                    </span>
                    ${calc.clockedIn && calc.lastEventTime
                        ? `<span class="muted">since ${escapeHtml(formatClock(calc.lastEventTime * 1000))}</span>`
                        : ""}
                </div>
                <div class="status-time">${escapeHtml(formatMinutes(calc.workedToday))}</div>
                <div class="status-sub">
                    Worked today · target ${escapeHtml(formatMinutes(calc.todayTarget))}
                </div>
                <div class="status-balance">
                    <span>Flexi balance</span>
                    <strong class="${balanceClass}">
                        ${calc.flexiUsed ? escapeHtml(formatMinutes(calc.balance)) : "off"}
                    </strong>
                </div>

                <div class="clock-actions">
                    ${calc.clockedIn
                        ? `<button class="btn danger wide" data-action="clock-out">Clock out</button>`
                        : `
                        <div class="clock-fields">
                            <label>Task
                                <select data-clock-task>
                                    ${taskOptionsHtml(state, state.clock.taskId)}
                                </select>
                            </label>
                            <label>Comment
                                <input data-clock-comment value="${escapeHtml(state.clock.comment)}"
                                       placeholder="optional" maxlength="500">
                            </label>
                        </div>
                        <button class="btn primary wide" data-action="clock-in">Clock in</button>
                        `}
                </div>
            </section>

            <section class="card week-card">
                <div class="card-head">
                    <h2>Week ${week ? week.weekNumber : ""}</h2>
                    <div class="btn-group" role="group" aria-label="Week navigation">
                        <button class="btn small ghost" data-week="-7" title="Previous week">‹</button>
                        <button class="btn small ghost" data-week="0">This week</button>
                        <button class="btn small ghost" data-week="7" title="Next week">›</button>
                    </div>
                </div>
                ${week ? weekTableHtml(week, state.settings.flexiEnabled) : emptyState("No week data.")}
            </section>

            <section class="card today-card">
                <div class="card-head">
                    <h2>Today</h2>
                    <button class="btn small ghost" data-action="all-events">All events</button>
                </div>
                ${todayEvents.length
                    ? `<table class="data-table compact">
                        <thead><tr><th>Time</th><th>Type</th><th>Task</th></tr></thead>
                        <tbody>
                        ${todayEvents.map((event) => `
                            <tr>
                                <td class="mono">${escapeHtml(formatClock(event.time * 1000))}</td>
                                <td>${eventTypeBadge(event.type)}</td>
                                <td>${event.task ? escapeHtml(taskName(state, event.task)) : '<span class="muted">—</span>'}</td>
                            </tr>
                        `).join("")}
                        </tbody>
                    </table>`
                    : emptyState("No events today.")}
            </section>
        </div>
    `;

    root.querySelector('[data-action="refresh"]')?.addEventListener("click", () => ctx.refresh());

    root.querySelectorAll("[data-week]").forEach((button) => {
        button.addEventListener("click", () => {
            const delta = Number(button.dataset.week);
            if (delta === 0) {
                state.weekFollowsToday = true;
                state.weekStartDay = mondayOf(localEpochDay(Date.now()));
            } else {
                state.weekFollowsToday = false;
                state.weekStartDay += delta;
            }
            ctx.recompute();
            renderDashboard(root, ctx);
        });
    });

    const taskSelect = root.querySelector("[data-clock-task]");
    const commentInput = root.querySelector("[data-clock-comment]");
    taskSelect?.addEventListener("change", () => {
        state.clock.taskId = taskSelect.value;
    });
    commentInput?.addEventListener("input", () => {
        state.clock.comment = commentInput.value;
    });

    root.querySelector('[data-action="clock-in"]')?.addEventListener("click", async (event) => {
        const button = event.currentTarget;
        const now = Date.now();
        const taskId = taskSelect ? taskSelect.value : state.clock.taskId;
        state.clock.taskId = taskId;
        const comment = commentInput ? commentInput.value : state.clock.comment;
        setBusy(button, true, "Clocking in…");
        try {
            await ctx.mutate(
                () => ctx.api.createEvent({
                    time: Math.floor(now / 1000),
                    zoneOffset: -new Date(now).getTimezoneOffset(),
                    type: "CLOCK_IN",
                    task: taskId || null,
                    comment: comment.trim(),
                }),
                "Clocked in.",
            );
            state.clock.comment = "";
        } catch {
            // ctx.mutate already surfaced the error
        } finally {
            setBusy(button, false);
        }
    });

    root.querySelector('[data-action="clock-out"]')?.addEventListener("click", async (event) => {
        const button = event.currentTarget;
        const now = Date.now();
        setBusy(button, true, "Clocking out…");
        try {
            await ctx.mutate(
                () => ctx.api.createEvent({
                    time: Math.floor(now / 1000),
                    zoneOffset: -new Date(now).getTimezoneOffset(),
                    type: "CLOCK_OUT",
                    task: null,
                    comment: "",
                }),
                "Clocked out.",
            );
        } catch {
            // surfaced by ctx.mutate
        } finally {
            setBusy(button, false);
        }
    });

    root.querySelector('[data-action="all-events"]')?.addEventListener("click", () => {
        ctx.navigate("events");
    });
}

const DASH = '<span class="muted">—</span>';

function totalCell(minutes) {
    return minutes === null || minutes === undefined
        ? DASH
        : escapeHtml(formatMinutes(minutes));
}

function weekTableHtml(week, showBalance) {
    const rows = week.days.map((day) => {
        const label = `${formatWeekday(day.epochDay)} ${formatEpochDayShort(day.epochDay)}`;
        const classes = [day.isToday ? "today" : "", day.future ? "future" : ""].join(" ").trim();
        const dash = DASH;
        return `
            <tr class="${classes}">
                <th scope="row">${escapeHtml(label)}</th>
                <td class="mono">${day.inMs === null ? dash : escapeHtml(formatClock(day.inMs))}</td>
                <td class="mono">${day.outMs === null ? dash : escapeHtml(formatClock(day.outMs))}</td>
                <td class="mono">${day.worked === null ? dash : escapeHtml(formatMinutes(day.worked))}</td>
                <td class="mono">${day.target === null ? dash : escapeHtml(formatMinutes(day.target))}</td>
                ${showBalance
                    ? `<td class="mono">${day.balance === null ? dash : escapeHtml(formatMinutes(day.balance))}</td>`
                    : ""}
            </tr>
        `;
    }).join("");

    return `
        <div class="table-wrap">
            <table class="data-table week-table">
                <thead>
                    <tr>
                        <th scope="col">Day</th>
                        <th scope="col">In</th>
                        <th scope="col">Out</th>
                        <th scope="col">Worked</th>
                        <th scope="col">Target</th>
                        ${showBalance ? '<th scope="col">Balance</th>' : ""}
                    </tr>
                </thead>
                <tbody>${rows}</tbody>
                <tfoot>
                    <tr>
                        <th scope="row">Total</th>
                        <td colspan="2"></td>
                        <td class="mono">${totalCell(week.totals.worked)}</td>
                        <td class="mono">${totalCell(week.totals.target)}</td>
                        ${showBalance
                            ? `<td class="mono">${totalCell(week.totals.balance)}</td>`
                            : ""}
                    </tr>
                </tfoot>
            </table>
        </div>
    `;
}
