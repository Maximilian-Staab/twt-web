// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// REST client for the twt-cloud backend.
//
// Units follow the API contract:
//   event time    — epoch seconds
//   zoneOffset    — UTC offset in minutes
//   target value  — minutes
//   target date   — YYYY-MM-DD
//
// Every request carries a fresh bearer token; a 401 triggers one forced
// refresh before the request is retried.

const HTTP_TIMEOUT_MS = 30000;
const EVENTS_PAGE_LIMIT = 1000;
const MAX_EVENTS = 50000;
const TARGETS_PAGE_LIMIT = 1000;
const MAX_TARGETS = 50000;

export class ApiError extends Error {
    constructor(status, problem, statusText) {
        const detail = problem && (problem.detail || problem.title);
        super(detail || `HTTP ${status}${statusText ? ` ${statusText}` : ""}`);
        this.name = "ApiError";
        this.status = status;
        this.problem = problem || null;
    }
}

export class ApiClient {
    constructor({apiBase, auth, fetchImpl = window.fetch.bind(window)}) {
        this.apiBase = apiBase.replace(/\/+$/, "");
        this.auth = auth;
        this.fetchImpl = fetchImpl;
    }

    async request(path, {method = "GET", query, body, headers = {}, retry = true} = {}) {
        const token = await this.auth.accessToken();
        const url = this.#url(path, query);
        const init = {
            method,
            headers: {
                Accept: "application/json",
                Authorization: `Bearer ${token}`,
                ...headers,
            },
            signal: timeoutSignal(),
        };
        if (body !== undefined) {
            init.headers["Content-Type"] = "application/json";
            init.body = JSON.stringify(body);
        }

        let response;
        try {
            response = await this.fetchImpl(url, init);
        } catch (e) {
            throw new ApiError(0, {
                title: "Network error",
                detail: `Network error: ${e.message}`,
            });
        }

        if (response.status === 401 && retry) {
            await this.auth.forceRefresh();
            return this.request(path, {method, query, body, headers, retry: false});
        }

        if (!response.ok) {
            throw new ApiError(response.status, await readProblem(response), response.statusText);
        }
        if (response.status === 204) {
            // Consume the (empty) body; an unread response stream makes
            // Chromium report the request as net::ERR_ABORTED on GC.
            await response.text();
            return null;
        }
        const contentType = response.headers.get("content-type") || "";
        if (contentType.includes("application/json")) {
            return response.json();
        }
        return response.text();
    }

    get(path, query) {
        return this.request(path, {query});
    }

    post(path, body, query) {
        return this.request(path, {method: "POST", body, query});
    }

    patch(path, body, query) {
        return this.request(path, {method: "PATCH", body, query});
    }

    delete(path, query) {
        return this.request(path, {method: "DELETE", query});
    }

    // ---- convenience endpoints ----

    listTasks() {
        return this.get("/tasks");
    }

    createTask(task) {
        return this.post("/tasks", task);
    }

    updateTask(id, patch) {
        return this.patch(`/tasks/${id}`, patch);
    }

    deleteTask(id, updatedAt) {
        return this.delete(`/tasks/${id}`, {updatedAt});
    }

    /** Fetches every target via the backend's limit/offset pagination. */
    async listAllTargets() {
        const targets = [];
        for (let offset = 0; offset < MAX_TARGETS; offset += TARGETS_PAGE_LIMIT) {
            const page = await this.get("/targets", {
                limit: TARGETS_PAGE_LIMIT,
                offset,
            });
            targets.push(...page);
            if (page.length < TARGETS_PAGE_LIMIT) {
                return targets;
            }
        }
        throw new ApiError(0, {
            title: "Too many targets",
            detail: `Refusing to load more than ${MAX_TARGETS} targets.`,
        });
    }

    createTarget(target) {
        return this.post("/targets", target);
    }

    updateTarget(id, patch) {
        return this.patch(`/targets/${id}`, patch);
    }

    deleteTarget(id, updatedAt) {
        return this.delete(`/targets/${id}`, {updatedAt});
    }

    /** Fetches every event (the calculations need the full history). */
    async listAllEvents() {
        const events = [];
        for (let offset = 0; offset < MAX_EVENTS; offset += EVENTS_PAGE_LIMIT) {
            const page = await this.get("/events", {
                limit: EVENTS_PAGE_LIMIT,
                offset,
            });
            events.push(...page);
            if (page.length < EVENTS_PAGE_LIMIT) {
                return events;
            }
        }
        throw new ApiError(0, {
            title: "Too many events",
            detail: `Refusing to load more than ${MAX_EVENTS} events.`,
        });
    }

    createEvent(event) {
        return this.post("/events", event);
    }

    updateEvent(id, event) {
        return this.patch(`/events/${id}`, event);
    }

    deleteEvent(id, updatedAt) {
        return this.delete(`/events/${id}`, {updatedAt});
    }

    #url(path, query) {
        let url = `${this.apiBase}${path.startsWith("/") ? path : `/${path}`}`;
        if (query) {
            const params = new URLSearchParams();
            for (const [key, value] of Object.entries(query)) {
                if (value === undefined || value === null || value === "") continue;
                params.set(key, String(value));
            }
            const qs = params.toString();
            if (qs) url += `?${qs}`;
        }
        return url;
    }
}

async function readProblem(response) {
    try {
        const text = await response.text();
        if (!text) return null;
        return JSON.parse(text);
    } catch {
        return null;
    }
}

function timeoutSignal() {
    return typeof AbortSignal !== "undefined" && AbortSignal.timeout
        ? AbortSignal.timeout(HTTP_TIMEOUT_MS)
        : undefined;
}
