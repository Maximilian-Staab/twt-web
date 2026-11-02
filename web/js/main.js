// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// Web frontend entrypoint: auth gate, shell, data refresh, and view routing.
//
// Data flow mirrors the twtBar widget: fetch the full event/target history,
// compute worked-today and flexi balance client-side, then render. Every
// mutation goes through the REST API and triggers a refresh.

import { Auth, AuthError, callbackUrl, normalizeApiBase } from "./auth.js";
import { ApiClient, ApiError } from "./api.js";
import { calculate, calculateWeek } from "./engine.js";
import {
    formatClock,
    homeOffsetFor,
    localEpochDay,
    mondayOf,
} from "./format.js";
import {
    API_BASE_KEY,
    defaultApiBase,
    engineSettings,
    loadSettings,
} from "./settings.js";
import { confirmDialog, escapeHtml, formatError, setBusy, showToast } from "./ui.js";
import { activeTasks, defaultTaskId } from "./views/common.js";
import { renderDashboard } from "./views/dashboard.js";
import { renderEvents } from "./views/events.js";
import { renderTasks } from "./views/tasks.js";
import { renderTargets } from "./views/targets.js";
import { renderSettings } from "./views/settings.js";

const VIEWS = ["dashboard", "events", "tasks", "targets", "settings"];
const VIEW_RENDERERS = {
    dashboard: renderDashboard,
    events: renderEvents,
    tasks: renderTasks,
    targets: renderTargets,
    settings: renderSettings,
};
const AUTO_REFRESH_MS = 60000;

const app = document.getElementById("app");
const auth = new Auth();
let api = null;
let refreshPromise = null;
let refreshTimer = null;

const state = {
    apiBase: defaultApiBase(),
    settings: loadSettings(),
    tasks: [],
    events: [],
    targets: [],
    calc: null,
    week: null,
    weekStartDay: mondayOf(localEpochDay(Date.now())),
    weekFollowsToday: true,
    view: "dashboard",
    eventRangeDays: 30,
    clock: {taskId: null, comment: ""},
    lastUpdated: null,
    loading: false,
    initialized: false,
    loadError: "",
    renderPending: false,
};

const ctx = {
    auth,
    get api() {
        return api;
    },
    state,
    refresh: () => refreshData(),
    mutate: async (fn, message) => {
        try {
            await fn();
            if (message) showToast(message, "ok");
            // A refresh that started before the write may resolve with a
            // pre-mutation snapshot. Let it settle, then fetch again.
            if (refreshPromise) {
                await refreshPromise;
            }
            await refreshData();
        } catch (error) {
            handleError(error);
            throw error;
        }
    },
    recompute,
    navigate,
    signOut: () => signOut({message: "Signed out."}),
    changeServer,
    toast: showToast,
};

boot();

async function boot() {
    state.apiBase = readStoredApiBase();

    // `?api=<url>` asks the app to use another backend. It is untrusted input:
    // applying it silently would let a link point the sign-in (and, with the
    // token proxy, the authorization code) at a host the user never chose.
    // Confirm the change explicitly and persist it only when accepted.
    let apiError = "";
    const requestedApi = new URLSearchParams(window.location.search).get("api");
    if (requestedApi !== null) {
        try {
            const requested = normalizeApiBase(requestedApi);
            if (requested !== state.apiBase) {
                if (await confirmServerChange(requested, state.apiBase)) {
                    state.apiBase = requested;
                    storeApiBase(requested);
                }
            }
        } catch (error) {
            apiError = formatError(error);
        } finally {
            clearApiParam();
        }
    }

    state.settings = loadSettings();
    state.weekStartDay = mondayOf(localEpochDay(Date.now()));
    const requestedView = window.location.hash.slice(1);
    if (VIEWS.includes(requestedView)) {
        state.view = requestedView;
    }

    let callbackError = null;
    try {
        const established = await auth.handleCallback();
        if (established) {
            state.apiBase = auth.apiBase;
            storeApiBase(state.apiBase);
            showToast(`Signed in${auth.subject ? ` as ${auth.subject}` : ""}.`, "ok");
        }
    } catch (error) {
        callbackError = error;
        auth.signOut();
    }

    if (auth.isSignedIn && auth.apiBase === state.apiBase) {
        api = new ApiClient({apiBase: state.apiBase, auth});
        renderShell();
        startAutoRefresh();
        if (apiError) {
            showToast(apiError, "error");
        }
        await refreshData();
    } else {
        if (auth.isSignedIn) {
            // Stored session belongs to another backend URL.
            auth.signOut();
        }
        const loginErrors = [apiError, callbackError ? formatError(callbackError) : ""].filter(Boolean);
        renderLogin(loginErrors.join(" · "));
    }

    window.addEventListener("hashchange", onHashChange);
    document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible" && api) {
            refreshData();
        }
    });
}

/**
 * A link can name any backend in `?api=`. Show the target explicitly and
 * default to keeping the current one; only an affirmative answer persists it.
 */
async function confirmServerChange(requested, current) {
    return confirmDialog(
        `This link asks the app to use ${requested} instead of ${current}. ` +
        `Any current session will be signed out, and signing in will send your ` +
        `authorization code to the new host. Only continue if you trust it.`,
        {
            title: "Use a different backend?",
            confirmLabel: "Use this server",
            cancelLabel: "Keep current server",
            danger: false,
            focusConfirm: false,
        },
    );
}

/** Drops `api` from the address bar so a reload does not ask again. */
function clearApiParam() {
    const url = new URL(window.location.href);
    if (!url.searchParams.has("api")) return;
    url.searchParams.delete("api");
    try {
        window.history.replaceState(null, "", url.toString());
    } catch {
        // history may be locked down in exotic embeddings; harmless
    }
}

// ---------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------

function renderLogin(message = "") {
    document.title = "Sign in · TrackWorkTime";
    const redirect = callbackUrl();
    app.innerHTML = `
        <div class="login-wrap">
            <section class="card login-card">
                <div class="brand brand-large">
                    <span class="brand-icon" aria-hidden="true">◷</span>
                    <span>TrackWorkTime</span>
                </div>
                <p class="muted">
                    Sign in through your OIDC provider using OAuth 2.0
                    Authorization Code + PKCE — the same flow as the Android app
                    and the twtBar plugin.
                </p>
                <label>Backend URL
                    <input id="api-base" type="url" spellcheck="false"
                           value="${escapeHtml(state.apiBase)}"
                           placeholder="${escapeHtml(defaultApiBase())}">
                    <span class="hint">Server root or a URL ending in /api/v1.</span>
                </label>
                <button id="signin" class="btn primary wide">Sign in</button>
                <p id="login-error" class="form-error login-error" ${message ? "" : "hidden"}>
                    ${escapeHtml(message)}
                </p>
                <p class="hint login-hint">
                    The OIDC client must allow <code>${escapeHtml(redirect)}</code>
                    as a redirect URI and its token endpoint must allow CORS for
                    this origin. If the provider reports <code>invalid redirect_uri</code>,
                    open this page at exactly that URL (for example
                    <code>127.0.0.1</code> rather than <code>localhost</code>).
                </p>
            </section>
        </div>
    `;

    const input = app.querySelector("#api-base");
    const button = app.querySelector("#signin");
    const errorEl = app.querySelector("#login-error");

    const submit = async () => {
        errorEl.hidden = true;
        setBusy(button, true, "Redirecting…");
        try {
            const base = normalizeApiBase(input.value);
            state.apiBase = base;
            storeApiBase(base);
            auth.signOut();
            // Navigates to the IdP; this promise does not resolve.
            await auth.beginSignIn(base);
        } catch (error) {
            errorEl.textContent = formatError(error);
            errorEl.hidden = false;
            setBusy(button, false);
        }
    };
    button.addEventListener("click", submit);
    input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
            event.preventDefault();
            submit();
        }
    });
}

// ---------------------------------------------------------------------------
// shell and routing
// ---------------------------------------------------------------------------

function renderShell() {
    document.title = "TrackWorkTime";
    app.innerHTML = `
        <header class="topbar">
            <div class="brand">
                <span class="brand-icon" aria-hidden="true">◷</span>
                <span>TrackWorkTime</span>
            </div>
            <nav class="tabs" aria-label="Sections">
                ${VIEWS.map((view) => `
                    <button class="tab" data-view="${view}">
                        ${view[0].toUpperCase() + view.slice(1)}
                    </button>
                `).join("")}
            </nav>
            <div class="topbar-right">
                <span id="clock-indicator" class="clock-indicator">
                    <span class="dot"></span><span class="label">—</span>
                </span>
                <span id="sync-status" class="sync-status muted"></span>
                <span id="user-sub" class="user-sub" title="Signed-in subject"></span>
                <button id="refresh-button" class="btn small ghost">Refresh</button>
                <button id="signout-button" class="btn small ghost">Sign out</button>
            </div>
        </header>
        <main id="view-root" class="view-root"></main>
    `;

    app.querySelectorAll("[data-view]").forEach((button) => {
        button.addEventListener("click", () => navigate(button.dataset.view));
    });
    app.querySelector("#refresh-button").addEventListener("click", () => refreshData());
    app.querySelector("#signout-button").addEventListener("click", () => ctx.signOut());

    app.querySelector("#view-root").addEventListener("focusout", () => {
        if (!state.renderPending) return;
        // focusout fires before the next element is focused; decide after it.
        setTimeout(() => {
            if (state.renderPending && !isTypingInView()) {
                renderCurrentView();
            }
        }, 0);
    });

    updateShell();
    renderCurrentView();
}

function onHashChange() {
    const view = window.location.hash.slice(1);
    if (!VIEWS.includes(view)) return;
    state.view = view;
    updateShell();
    renderCurrentView();
}

function navigate(view) {
    if (!VIEWS.includes(view)) return;
    if (window.location.hash === `#${view}`) {
        state.view = view;
        updateShell();
        renderCurrentView();
        return;
    }
    window.location.hash = view;
}

function updateShell() {
    if (!document.getElementById("view-root")) return;

    app.querySelectorAll("[data-view]").forEach((button) => {
        button.classList.toggle("active", button.dataset.view === state.view);
        button.setAttribute("aria-current", button.dataset.view === state.view ? "page" : "false");
    });

    const subject = document.getElementById("user-sub");
    if (subject) subject.textContent = auth.subject || "signed in";

    const clock = document.getElementById("clock-indicator");
    if (clock) {
        const clockedIn = Boolean(state.calc && state.calc.clockedIn);
        clock.classList.toggle("in", clockedIn);
        clock.querySelector(".label").textContent = clockedIn ? "Clocked in" : "Clocked out";
    }

    const sync = document.getElementById("sync-status");
    if (sync) {
        sync.textContent = state.loading
            ? "Syncing…"
            : state.lastUpdated
                ? `Updated ${formatClock(state.lastUpdated)}`
                : "";
    }

    const refreshButton = document.getElementById("refresh-button");
    if (refreshButton) refreshButton.disabled = state.loading;
}

function isTypingInView() {
    const active = document.activeElement;
    return Boolean(
        active
        && active.matches("input, select, textarea")
        && active.closest("#view-root"),
    );
}

function renderCurrentView() {
    const root = document.getElementById("view-root");
    if (!root) return;
    state.renderPending = false;
    if (!state.initialized && state.view !== "settings") {
        if (state.loadError) {
            root.innerHTML = `
                <div class="card loading-card">
                    <div>
                        <strong>Could not load data.</strong>
                        <p class="muted">${escapeHtml(state.loadError)}</p>
                    </div>
                    <button class="btn ghost" data-retry>Retry</button>
                </div>
            `;
            root.querySelector("[data-retry]")?.addEventListener("click", () => refreshData());
        } else {
            root.innerHTML = `
                <div class="card loading-card">
                    <span class="spinner" aria-hidden="true"></span>
                    Loading events, tasks and targets…
                </div>
            `;
        }
        return;
    }
    const render = VIEW_RENDERERS[state.view] || renderDashboard;
    render(root, ctx);
}

// ---------------------------------------------------------------------------
// data
// ---------------------------------------------------------------------------

function refreshData() {
    if (!api) return Promise.resolve();
    if (refreshPromise) return refreshPromise;

    refreshPromise = (async () => {
        state.loading = true;
        updateShell();
        try {
            const [tasks, events, targets] = await Promise.all([
                api.listTasks(),
                api.listAllEvents(),
                api.listAllTargets(),
            ]);
            state.tasks = tasks;
            state.events = events;
            state.targets = targets;
            state.lastUpdated = Date.now();
            state.initialized = true;
            state.loadError = "";

            if (
                state.clock.taskId === null ||
                (state.clock.taskId !== "" &&
                    !activeTasks(state).some((t) => t.id === state.clock.taskId))
            ) {
                state.clock.taskId = defaultTaskId(state);
            }

            recompute();
            updateShell();
            if (state.view !== "settings") {
                // Re-rendering the view replaces its inputs; if the user is
                // typing (the clock-in comment, a filter), keep the DOM and
                // render as soon as the field loses focus.
                if (isTypingInView()) {
                    state.renderPending = true;
                } else {
                    renderCurrentView();
                }
            }
        } catch (error) {
            state.loadError = formatError(error);
            handleError(error);
        } finally {
            state.loading = false;
            refreshPromise = null;
            updateShell();
            if (!state.initialized) {
                renderCurrentView();
            }
        }
    })();
    return refreshPromise;
}

function recompute() {
    const now = Date.now();
    if (state.weekFollowsToday) {
        // A long-lived tab must roll over to the new week at midnight.
        state.weekStartDay = mondayOf(localEpochDay(now));
    }
    const settings = engineSettings(state.settings);
    state.calc = calculate(state.events, state.targets, settings, now, homeOffsetFor);
    state.week = calculateWeek(
        state.events,
        state.targets,
        settings,
        now,
        homeOffsetFor,
        state.weekStartDay,
    );
}

function startAutoRefresh() {
    clearInterval(refreshTimer);
    refreshTimer = setInterval(() => {
        if (!api || document.visibilityState !== "visible") return;
        refreshData();
    }, AUTO_REFRESH_MS);
}

// ---------------------------------------------------------------------------
// session actions
// ---------------------------------------------------------------------------

function signOut({message = ""} = {}) {
    auth.signOut();
    api = null;
    state.events = [];
    state.tasks = [];
    state.targets = [];
    state.calc = null;
    state.week = null;
    state.initialized = false;
    state.loadError = "";
    state.lastUpdated = null;
    state.weekFollowsToday = true;
    state.renderPending = false;
    state.clock = {taskId: null, comment: ""};
    if (message) showToast(message, "info");
    renderLogin();
}

function changeServer(base) {
    try {
        storeApiBase(base);
    } catch {
        // storage unavailable: the reload falls back to the default
    }
    auth.signOut();
    window.location.reload();
}

function handleError(error) {
    if (
        (error instanceof AuthError && (error.code === "expired" || error.code === "not_signed_in")) ||
        (error instanceof ApiError && error.status === 401)
    ) {
        signOut({message: error.message || "Session expired — sign in again."});
        return;
    }
    showToast(formatError(error), "error");
}

// ---------------------------------------------------------------------------
// storage helpers
// ---------------------------------------------------------------------------

function readStoredApiBase() {
    try {
        return normalizeApiBase(localStorage.getItem(API_BASE_KEY) || defaultApiBase());
    } catch {
        return defaultApiBase();
    }
}

function storeApiBase(base) {
    try {
        localStorage.setItem(API_BASE_KEY, base);
    } catch {
        // ignore
    }
}
