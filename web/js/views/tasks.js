// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Tasks: create, rename, activate/deactivate, set the default task, delete.

import { escapeHtml, openDialog, confirmDialog } from "../ui.js";
import { formatDateTime } from "../format.js";
import { emptyState } from "./common.js";

export function renderTasks(root, ctx) {
    const {state} = ctx;
    const tasks = state.tasks;

    root.innerHTML = `
        <section class="view-head">
            <div>
                <h1>Tasks</h1>
                <p class="muted">Tasks group clock-in events; the default task is preselected.</p>
            </div>
            <button class="btn primary" data-action="add-task">New task</button>
        </section>

        <section class="card">
            ${tasks.length ? `
                <div class="table-wrap">
                    <table class="data-table">
                        <thead>
                            <tr>
                                <th scope="col">Name</th>
                                <th scope="col">Status</th>
                                <th scope="col">Default</th>
                                <th scope="col">Updated</th>
                                <th scope="col"><span class="sr-only">Actions</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            ${tasks.map((task) => `
                                <tr>
                                    <td>${escapeHtml(task.name)}</td>
                                    <td><span class="badge ${task.active ? "badge-on" : "badge-off"}">
                                        ${task.active ? "active" : "inactive"}
                                    </span></td>
                                    <td>${task.isDefault ? '<span class="badge badge-default">default</span>' : '<span class="muted">—</span>'}</td>
                                    <td class="muted">${escapeHtml(formatDateTime(Date.parse(task.updatedAt)))}</td>
                                    <td class="actions">
                                        <button class="btn small ghost" data-action="rename-task" data-id="${escapeHtml(task.id)}">Rename</button>
                                        <button class="btn small ghost" data-action="toggle-task" data-id="${escapeHtml(task.id)}">
                                            ${task.active ? "Disable" : "Enable"}
                                        </button>
                                        ${task.isDefault ? "" : `
                                        <button class="btn small ghost" data-action="default-task" data-id="${escapeHtml(task.id)}">
                                            Make default
                                        </button>`}
                                        <button class="btn small ghost danger-text" data-action="delete-task" data-id="${escapeHtml(task.id)}">
                                            Delete
                                        </button>
                                    </td>
                                </tr>
                            `).join("")}
                        </tbody>
                    </table>
                </div>
            ` : emptyState("No tasks yet. Create one to start clocking in with a task.")}
        </section>
    `;

    root.querySelector('[data-action="add-task"]')?.addEventListener("click", () => {
        openTaskDialog(ctx, null);
    });

    root.querySelector("tbody")?.addEventListener("click", async (event) => {
        const button = event.target.closest("button[data-action]");
        if (!button) return;
        const task = state.tasks.find((t) => t.id === button.dataset.id);
        if (!task) return;
        button.disabled = true;
        try {
            switch (button.dataset.action) {
                case "rename-task":
                    openTaskDialog(ctx, task);
                    break;
                case "toggle-task":
                    await ctx.mutate(
                        () => ctx.api.updateTask(task.id, {
                            active: !task.active,
                            updatedAt: task.updatedAt,
                        }),
                        task.active ? "Task disabled." : "Task enabled.",
                    );
                    break;
                case "default-task":
                    await ctx.mutate(async () => {
                        await clearDefaultTasks(ctx, task.id);
                        await ctx.api.updateTask(task.id, {
                            isDefault: true,
                            updatedAt: task.updatedAt,
                        });
                    }, "Default task updated.");
                    break;
                case "delete-task":
                    await deleteTask(ctx, task);
                    break;
            }
        } catch {
            // ctx.mutate already surfaced the error
        } finally {
            button.disabled = false;
        }
    });
}

function openTaskDialog(ctx, task) {
    const isEdit = Boolean(task);
    openDialog({
        title: isEdit ? "Rename task" : "New task",
        submitLabel: isEdit ? "Save" : "Create task",
        bodyHtml: `
            <label>Name
                <input type="text" name="name" required maxlength="200"
                       value="${escapeHtml(task ? task.name : "")}"
                       placeholder="e.g. Project A">
            </label>
            ${isEdit ? "" : `
            <div class="checks">
                <label class="check"><input type="checkbox" name="active" checked> Active</label>
                <label class="check"><input type="checkbox" name="isDefault"> Make default</label>
            </div>`}
        `,
        onSubmit: async (formData) => {
            const name = String(formData.get("name") || "").trim();
            if (!name) {
                throw new Error("Task name is required.");
            }
            if (isEdit) {
                if (name === task.name) return;
                await ctx.mutate(
                    () => ctx.api.updateTask(task.id, {
                        name,
                        updatedAt: task.updatedAt,
                    }),
                    "Task renamed.",
                );
                return;
            }
            const makeDefault = formData.get("isDefault") === "on";
            await ctx.mutate(async () => {
                if (makeDefault) {
                    await clearDefaultTasks(ctx, null);
                }
                await ctx.api.createTask({
                    name,
                    active: formData.get("active") === "on",
                    isDefault: makeDefault,
                });
            }, "Task created.");
        },
    });
}

async function clearDefaultTasks(ctx, exceptId) {
    const defaults = ctx.state.tasks.filter(
        (t) => t.isDefault && t.id !== exceptId,
    );
    for (const task of defaults) {
        await ctx.api.updateTask(task.id, {
            isDefault: false,
            updatedAt: task.updatedAt,
        });
    }
}

async function deleteTask(ctx, task) {
    const {state} = ctx;
    if (state.tasks.length <= 1) {
        ctx.toast("Cannot delete the last task — rename it instead.", "error");
        return;
    }
    const used = state.events.some((event) => !event.deletedAt && event.task === task.id);
    if (used) {
        ctx.toast("This task is used by events and cannot be deleted.", "error");
        return;
    }
    const ok = await confirmDialog(
        `Delete task “${task.name}”?`,
        {title: "Delete task", confirmLabel: "Delete"},
    );
    if (!ok) return;
    await ctx.mutate(
        () => ctx.api.deleteTask(task.id, task.updatedAt),
        "Task deleted.",
    );
}
