// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Small DOM helpers: escaping, toasts, modal dialogs, busy buttons.

export function escapeHtml(value) {
    return String(value ?? "")
        .replaceAll("&", "&amp;")
        .replaceAll("<", "&lt;")
        .replaceAll(">", "&gt;")
        .replaceAll('"', "&quot;")
        .replaceAll("'", "&#39;");
}

export function showToast(message, kind = "info", timeoutMs = 4500) {
    let toast = document.getElementById("toast");
    if (!toast) {
        toast = document.createElement("div");
        toast.id = "toast";
        toast.setAttribute("role", "status");
        toast.setAttribute("aria-live", "polite");
        document.body.append(toast);
    }
    toast.textContent = message;
    toast.className = `toast toast-${kind} visible`;
    clearTimeout(showToast.timer);
    showToast.timer = setTimeout(() => {
        toast.classList.remove("visible");
    }, timeoutMs);
}

export function formatError(error) {
    if (!error) return "Unknown error";
    if (error.problem && (error.problem.detail || error.problem.title)) {
        const detail = error.problem.detail || error.problem.title;
        return error.status ? `${detail} (HTTP ${error.status})` : detail;
    }
    return error.message || String(error);
}

/**
 * Opens a modal form. `onSubmit(formData, form)` may return a promise;
 * an error keeps the dialog open and shows the message inline.
 */
export function openDialog({
    title,
    bodyHtml,
    submitLabel = "Save",
    cancelLabel = "Cancel",
    danger = false,
    onSubmit = null,
    onMount = null,
}) {
    const dialog = document.createElement("dialog");
    dialog.className = "dialog";
    dialog.innerHTML = `
        <form class="dialog-form" novalidate>
            <h2 class="dialog-title">${escapeHtml(title)}</h2>
            <div class="dialog-body">${bodyHtml}</div>
            <p class="form-error" data-error hidden></p>
            <div class="dialog-actions">
                <button type="button" class="btn ghost" data-cancel>${escapeHtml(cancelLabel)}</button>
                <button type="submit" class="btn ${danger ? "danger" : "primary"}">${escapeHtml(submitLabel)}</button>
            </div>
        </form>
    `;
    document.body.append(dialog);

    const form = dialog.querySelector("form");
    const errorEl = dialog.querySelector("[data-error]");
    const cancelBtn = dialog.querySelector("[data-cancel]");
    const submitBtn = dialog.querySelector('button[type="submit"]');

    cancelBtn.addEventListener("click", () => dialog.close());
    dialog.addEventListener("click", (event) => {
        if (event.target === dialog) dialog.close();
    });
    dialog.addEventListener("close", () => dialog.remove(), {once: true});
    form.addEventListener("submit", async (event) => {
        event.preventDefault();
        if (!onSubmit) {
            dialog.close();
            return;
        }
        errorEl.hidden = true;
        submitBtn.disabled = true;
        try {
            await onSubmit(new FormData(form), form);
            dialog.close();
        } catch (error) {
            errorEl.textContent = formatError(error);
            errorEl.hidden = false;
        } finally {
            submitBtn.disabled = false;
        }
    });

    dialog.showModal();
    if (onMount) onMount(form, dialog);
    const first = form.querySelector("input:not([type=hidden]), select, textarea, button");
    if (first) first.focus();
    return dialog;
}

/** Promise-based confirm dialog. */
export function confirmDialog(message, {
    title = "Please confirm",
    confirmLabel = "Delete",
    cancelLabel = "Cancel",
    danger = true,
    focusConfirm = true,
} = {}) {
    return new Promise((resolve) => {
        let settled = false;
        const finish = (value) => {
            if (settled) return;
            settled = true;
            resolve(value);
        };
        const dialog = document.createElement("dialog");
        dialog.className = "dialog";
        dialog.innerHTML = `
            <div class="dialog-form">
                <h2 class="dialog-title">${escapeHtml(title)}</h2>
                <p class="dialog-message">${escapeHtml(message)}</p>
                <div class="dialog-actions">
                    <button type="button" class="btn ghost" data-cancel>${escapeHtml(cancelLabel)}</button>
                    <button type="button" class="btn ${danger ? "danger" : "primary"}" data-confirm>${escapeHtml(confirmLabel)}</button>
                </div>
            </div>
        `;
        document.body.append(dialog);
        dialog.querySelector("[data-cancel]").addEventListener("click", () => dialog.close());
        dialog.querySelector("[data-confirm]").addEventListener("click", () => {
            finish(true);
            dialog.close();
        });
        dialog.addEventListener("click", (event) => {
            if (event.target === dialog) dialog.close();
        });
        dialog.addEventListener("close", () => {
            finish(false);
            dialog.remove();
        }, {once: true});
        dialog.showModal();
        const focusTarget = focusConfirm ? "[data-confirm]" : "[data-cancel]";
        dialog.querySelector(focusTarget).focus();
    });
}

export function setBusy(button, busy, busyLabel = "Working…") {
    if (!button) return;
    if (busy) {
        if (!button.dataset.originalText) {
            button.dataset.originalText = button.textContent;
        }
        button.disabled = true;
        button.textContent = busyLabel;
    } else {
        button.disabled = false;
        if (button.dataset.originalText) {
            button.textContent = button.dataset.originalText;
            delete button.dataset.originalText;
        }
    }
}
