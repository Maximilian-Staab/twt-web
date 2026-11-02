// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Targets: day overrides/grants/ignores and flexi target changes.
// Values are stored as minutes, dates as YYYY-MM-DD (app semantics).

import { escapeHtml, openDialog, confirmDialog } from "../ui.js";
import {
    formatDateLabel,
    formatMinutes,
    localDateISO,
    parseDurationInput,
} from "../format.js";
import { emptyState, TARGET_TYPE_LABEL } from "./common.js";

const TARGET_TYPES = ["DAY_SET", "DAY_GRANT", "DAY_IGNORE", "FLEXI_SET", "FLEXI_ADD"];

export function renderTargets(root, ctx) {
    const {state} = ctx;
    const targets = [...state.targets].sort((a, b) => {
        if (a.date !== b.date) return a.date < b.date ? 1 : -1;
        return TARGET_TYPES.indexOf(a.type) - TARGET_TYPES.indexOf(b.type);
    });

    root.innerHTML = `
        <section class="view-head">
            <div>
                <h1>Targets</h1>
                <p class="muted">Day overrides and grants; flexi targets are stored but not used by the calculator.</p>
            </div>
            <button class="btn primary" data-action="add-target">New target</button>
        </section>

        <section class="card">
            ${targets.length ? `
                <div class="table-wrap">
                    <table class="data-table">
                        <thead>
                            <tr>
                                <th scope="col">Date</th>
                                <th scope="col">Type</th>
                                <th scope="col">Value</th>
                                <th scope="col">Comment</th>
                                <th scope="col"><span class="sr-only">Actions</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            ${targets.map((target) => `
                                <tr>
                                    <td>${escapeHtml(formatTargetDate(target.date))}</td>
                                    <td><span class="badge badge-type">${escapeHtml(TARGET_TYPE_LABEL[target.type] || target.type)}</span></td>
                                    <td class="mono">${target.type === "DAY_IGNORE" ||
                                        target.value === null || target.value === undefined
                                        ? '<span class="muted">—</span>'
                                        : escapeHtml(formatMinutes(target.value))}</td>
                                    <td class="comment-cell">${target.comment
                                        ? escapeHtml(target.comment)
                                        : '<span class="muted">—</span>'}</td>
                                    <td class="actions">
                                        <button class="btn small ghost" data-action="edit-target" data-id="${escapeHtml(target.id)}">Edit</button>
                                        <button class="btn small ghost danger-text" data-action="delete-target" data-id="${escapeHtml(target.id)}">Delete</button>
                                    </td>
                                </tr>
                            `).join("")}
                        </tbody>
                    </table>
                </div>
            ` : emptyState("No targets. Day targets override the regular weekly target.")}
        </section>
    `;

    root.querySelector('[data-action="add-target"]')?.addEventListener("click", () => {
        openTargetDialog(ctx, null);
    });

    root.querySelector("tbody")?.addEventListener("click", async (event) => {
        const button = event.target.closest("button[data-action]");
        if (!button) return;
        const target = state.targets.find((t) => t.id === button.dataset.id);
        if (!target) return;

        if (button.dataset.action === "edit-target") {
            openTargetDialog(ctx, target);
            return;
        }
        if (button.dataset.action === "delete-target") {
            const ok = await confirmDialog(
                `Delete the ${TARGET_TYPE_LABEL[target.type] || target.type} target for ${formatTargetDate(target.date)}?`,
                {title: "Delete target", confirmLabel: "Delete"},
            );
            if (!ok) return;
            button.disabled = true;
            try {
                await ctx.mutate(
                    () => ctx.api.deleteTarget(target.id, target.updatedAt),
                    "Target deleted.",
                );
            } catch {
                button.disabled = false;
            }
        }
    });
}

function openTargetDialog(ctx, target) {
    const isEdit = Boolean(target);
    const type = target ? target.type : "DAY_SET";
    openDialog({
        title: isEdit ? "Edit target" : "New target",
        submitLabel: isEdit ? "Save" : "Create target",
        bodyHtml: `
            <div class="form-grid">
                <label>Date
                    <input type="date" name="date" required
                           value="${escapeHtml(target ? target.date : localDateISO(Date.now()))}">
                </label>
                <label>Type
                    <select name="type">
                        ${TARGET_TYPES.map((value) =>
                            `<option value="${value}"${value === type ? " selected" : ""}>${TARGET_TYPE_LABEL[value]}</option>`,
                        ).join("")}
                    </select>
                </label>
            </div>
            <label data-value-field>Value
                <input type="text" name="value" inputmode="numeric" placeholder="8:00"
                       value="${escapeHtml(target && target.value !== null && target.value !== undefined
                           ? formatMinutes(target.value)
                           : "")}">
                <span class="hint">Target work time as H:MM.</span>
            </label>
            <label>Comment
                <input type="text" name="comment" maxlength="500"
                       value="${escapeHtml(target && target.comment ? target.comment : "")}"
                       placeholder="optional">
            </label>
        `,
        onSubmit: async (formData) => {
            const date = String(formData.get("date") || "");
            const targetType = String(formData.get("type"));
            if (!date) {
                throw new Error("Date is required.");
            }
            const payload = {
                date,
                type: targetType,
                comment: String(formData.get("comment") || "").trim(),
            };
            // DAY_IGNORE ignores `value`; omitting it also avoids pretending
            // the PATCH can clear an existing value (it cannot).
            if (targetType !== "DAY_IGNORE") {
                payload.value = parseDurationInput(formData.get("value"));
            }
            if (isEdit) {
                payload.updatedAt = target.updatedAt;
            }
            await ctx.mutate(
                () => isEdit
                    ? ctx.api.updateTarget(target.id, payload)
                    : ctx.api.createTarget(payload),
                isEdit ? "Target updated." : "Target created.",
            );
        },
        onMount: (form) => {
            const typeSelect = form.querySelector('select[name="type"]');
            const field = form.querySelector("[data-value-field]");
            const input = field.querySelector('input[name="value"]');
            const sync = () => {
                const ignored = typeSelect.value === "DAY_IGNORE";
                field.hidden = ignored;
                input.disabled = ignored;
            };
            typeSelect.addEventListener("change", sync);
            sync();
        },
    });
}

function formatTargetDate(date) {
    const ms = new Date(`${date}T00:00:00`).getTime();
    return Number.isNaN(ms) ? date : formatDateLabel(ms);
}
