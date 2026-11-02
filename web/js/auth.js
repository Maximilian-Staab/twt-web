// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 twt-cloud contributors

// OAuth 2.0 Authorization Code + PKCE for the web frontend.
//
// Mirrors the app's CloudAuthManager and the DMS plugin's twt_auth.py:
//   1. GET <apiBase>/config            — issuer, clientId, scopes, flow
//   2. GET <issuer>/.well-known/openid-configuration
//   3. browser round-trip with PKCE S256 on this page's URL as redirect_uri
//   4. POST token_endpoint             — authorization_code grant
//   5. refresh_token grant on demand
//
// The browser is the OAuth public client, so the IdP must allow CORS on its
// token endpoint and must have this page's URL registered as a redirect URI.
// Tokens are kept in localStorage; the in-flight PKCE verifier lives in
// sessionStorage so it is discarded with the tab.

const AUTH_KEY = "twt.auth.v1";
const PENDING_KEY = "twt.pendingAuth.v1";
const HTTP_TIMEOUT_MS = 15000;
const REFRESH_SKEW_MS = 60000;
const PENDING_MAX_AGE_MS = 10 * 60 * 1000;
const DEFAULT_EXPIRES_IN = 3600;

export class AuthError extends Error {
    constructor(message, code = "auth_error") {
        super(message);
        this.name = "AuthError";
        this.code = code;
    }
}

export class Auth {
    constructor({storage = window.localStorage, session = window.sessionStorage} = {}) {
        this.storage = storage;
        this.session = session;
        this.current = this.#readAuth();
        this.refreshPromise = null;
    }

    get isSignedIn() {
        return this.current !== null;
    }

    get apiBase() {
        return this.current ? this.current.apiBase : null;
    }

    get subject() {
        return this.current ? this.current.sub || "" : "";
    }

    get issuer() {
        return this.current ? this.current.issuer || "" : "";
    }

    /**
     * Starts the browser round-trip. On success the page navigates to the
     * IdP; the promise never resolves in the normal flow.
     */
    async beginSignIn(apiBase) {
        let config;
        try {
            config = await getJson(`${apiBase}/config`, "backend config");
        } catch (error) {
            throw new AuthError(
                `${error.message} — if the backend is reachable, add ` +
                `${window.location.origin} to CORS_ALLOWED_ORIGINS.`,
                error.code,
            );
        }
        const issuer = String(config.issuer || "").trim();
        const clientId = String(config.clientId || "").trim();
        const flow = String(config.flow || "authorization_code_pkce");

        if (!issuer) {
            throw new AuthError("Backend has no OAuth issuer configured (OAUTH_ISSUER_URL).");
        }
        if (!clientId) {
            throw new AuthError("Backend has no OAuth client id configured (OAUTH_CLIENT_ID).");
        }
        if (flow !== "authorization_code_pkce") {
            throw new AuthError(`Unsupported OAuth flow "${flow}".`);
        }
        if (!globalThis.crypto || !globalThis.crypto.subtle) {
            throw new AuthError("PKCE needs Web Crypto. Use HTTPS or localhost.");
        }

        const scopes = Array.isArray(config.scopes)
            ? config.scopes.map((s) => String(s).trim()).filter(Boolean)
            : [];
        if (!scopes.includes("offline_access")) {
            scopes.push("offline_access");
        }

        const discovery = await discover(apiBase, issuer);
        if (!discovery.authorization_endpoint || !discovery.token_endpoint) {
            throw new AuthError("OIDC discovery is missing authorization_endpoint or token_endpoint.");
        }
        // Newer backends relay the token request (`tokenProxy` in /config);
        // otherwise the browser posts to the IdP directly (requires CORS).
        const tokenEndpoint = config.tokenProxy === true
            ? `${apiBase}/oauth/token`
            : discovery.token_endpoint;

        const verifier = randomBase64Url(32);
        const challenge = await s256(verifier);
        const state = randomBase64Url(24);
        const nonce = randomBase64Url(24);
        const redirectUri = callbackUrl();

        this.session.setItem(PENDING_KEY, JSON.stringify({
            apiBase,
            issuer,
            clientId,
            tokenEndpoint,
            state,
            nonce,
            verifier,
            redirectUri,
            scope: scopes.join(" "),
            createdAt: Date.now(),
        }));

        const authUrl = new URL(discovery.authorization_endpoint);
        authUrl.searchParams.set("response_type", "code");
        authUrl.searchParams.set("client_id", clientId);
        authUrl.searchParams.set("redirect_uri", redirectUri);
        authUrl.searchParams.set("scope", scopes.join(" "));
        authUrl.searchParams.set("state", state);
        authUrl.searchParams.set("nonce", nonce);
        authUrl.searchParams.set("code_challenge", challenge);
        authUrl.searchParams.set("code_challenge_method", "S256");

        window.location.assign(authUrl.toString());
        // Keep the caller pending until the navigation happens.
        await new Promise(() => {});
    }

    /**
     * Handles the IdP redirect. Returns true when a session was established,
     * false when the current URL is not a callback, and throws on failure.
     * The URL is cleaned either way.
     */
    async handleCallback() {
        const params = new URLSearchParams(window.location.search);
        const code = params.get("code");
        const state = params.get("state");
        const error = params.get("error");
        if (!code && !error) {
            return false;
        }

        const cleanUrl = callbackUrl();
        const pending = this.#takePending();
        cleanCallbackUrl(cleanUrl);

        if (error) {
            const description = params.get("error_description") || "";
            throw new AuthError(`Authorization failed: ${error}${description ? ` — ${description}` : ""}`);
        }
        if (!pending) {
            throw new AuthError("Sign-in state is missing. Start the sign-in again.");
        }
        if (Date.now() - pending.createdAt > PENDING_MAX_AGE_MS) {
            throw new AuthError("Sign-in took too long. Start again.");
        }
        if (state !== pending.state) {
            throw new AuthError("OAuth state mismatch — aborting sign-in.");
        }

        const {status, data} = await postToken(pending.tokenEndpoint, {
            grant_type: "authorization_code",
            code,
            redirect_uri: pending.redirectUri,
            client_id: pending.clientId,
            code_verifier: pending.verifier,
        });
        if (status !== 200 || !data.access_token) {
            throw new AuthError(
                `Token exchange failed (HTTP ${status}): ${oauthErrorText(data)}`,
            );
        }

        // The ID token is only read for display, but the nonce we sent must
        // still come back unchanged: a mismatch means the response does not
        // belong to this sign-in attempt. Providers that issue no ID token
        // cannot be checked this way; PKCE + state still bind the exchange.
        const idToken = data.id_token || null;
        if (idToken && pending.nonce && readClaim(idToken, "nonce") !== pending.nonce) {
            throw new AuthError("OAuth nonce mismatch — aborting sign-in.");
        }

        this.current = {
            apiBase: pending.apiBase,
            issuer: pending.issuer,
            clientId: pending.clientId,
            tokenEndpoint: pending.tokenEndpoint,
            accessToken: data.access_token,
            refreshToken: data.refresh_token || null,
            idToken,
            expiresAt: Date.now() + expiresInMs(data),
            scope: data.scope || pending.scope,
            sub: readClaim(idToken, "sub"),
            signedInAt: Date.now(),
        };
        this.#writeAuth(this.current);
        return true;
    }

    /** Fresh access token, refreshing when it is about to expire. */
    async accessToken() {
        const auth = this.current;
        if (!auth) {
            throw new AuthError("Not signed in.", "not_signed_in");
        }
        if (auth.expiresAt - Date.now() > REFRESH_SKEW_MS) {
            return auth.accessToken;
        }
        return this.refresh();
    }

    /** Forces a refresh (used after an HTTP 401). */
    async forceRefresh() {
        return this.refresh();
    }

    refresh() {
        if (!this.refreshPromise) {
            this.refreshPromise = this.#refresh().finally(() => {
                this.refreshPromise = null;
            });
        }
        return this.refreshPromise;
    }

    async #refresh() {
        const auth = this.current;
        if (!auth) {
            throw new AuthError("Not signed in.", "not_signed_in");
        }
        if (!auth.refreshToken) {
            this.signOut();
            throw new AuthError("Session expired — sign in again.", "expired");
        }

        const {status, data} = await postToken(auth.tokenEndpoint, {
            grant_type: "refresh_token",
            refresh_token: auth.refreshToken,
            client_id: auth.clientId,
        });

        if (status === 200 && data.access_token) {
            auth.accessToken = data.access_token;
            auth.refreshToken = data.refresh_token || auth.refreshToken;
            auth.idToken = data.id_token || auth.idToken;
            auth.expiresAt = Date.now() + expiresInMs(data);
            if (data.scope) auth.scope = data.scope;
            this.#writeAuth(auth);
            return auth.accessToken;
        }

        if (status === 400 || status === 401 || status === 403) {
            // invalid_grant / invalid_client: the refresh token is permanently
            // unusable (expired, revoked, or the session ended at the IdP).
            this.signOut();
            throw new AuthError(
                `Sign-in expired — sign in again. (${oauthErrorText(data)})`,
                "expired",
            );
        }
        throw new AuthError(`Token refresh failed (HTTP ${status}).`, "refresh_failed");
    }

    signOut() {
        this.current = null;
        this.refreshPromise = null;
        try {
            this.storage.removeItem(AUTH_KEY);
            this.session.removeItem(PENDING_KEY);
        } catch {
            // storage unavailable — the in-memory state is gone either way
        }
    }

    #readAuth() {
        try {
            const raw = this.storage.getItem(AUTH_KEY);
            if (!raw) return null;
            const auth = JSON.parse(raw);
            if (!auth || !auth.apiBase || !auth.tokenEndpoint || !auth.accessToken) {
                this.storage.removeItem(AUTH_KEY);
                return null;
            }
            return auth;
        } catch {
            try {
                this.storage.removeItem(AUTH_KEY);
            } catch {
                // ignore
            }
            return null;
        }
    }

    #writeAuth(auth) {
        try {
            this.storage.setItem(AUTH_KEY, JSON.stringify(auth));
        } catch {
            // localStorage may be unavailable; tokens still work this session
        }
    }

    #takePending() {
        try {
            const raw = this.session.getItem(PENDING_KEY);
            this.session.removeItem(PENDING_KEY);
            return raw ? JSON.parse(raw) : null;
        } catch {
            return null;
        }
    }
}

/** The redirect URI for this deployment: origin + directory path. */
export function callbackUrl() {
    const url = new URL(window.location.href);
    url.search = "";
    url.hash = "";
    if (url.pathname.endsWith("/index.html")) {
        url.pathname = url.pathname.slice(0, -"index.html".length);
    }
    return url.toString();
}

function cleanCallbackUrl(cleanUrl) {
    try {
        window.history.replaceState(null, "", cleanUrl);
    } catch {
        // history may be locked down in exotic embeddings; harmless
    }
}

export function normalizeApiBase(raw) {
    let value = String(raw ?? "").trim();
    if (!value) {
        throw new AuthError("Enter the backend URL.");
    }
    // A bare host is the common case, but a protocol-relative URL or a
    // non-HTTP scheme must not be coerced into one: `//host` would become
    // `http:////host`, and `javascript:`/`file:` are never backends.
    if (value.startsWith("//")) {
        throw new AuthError("Enter a full backend URL starting with http:// or https://.");
    }
    if (/^[a-z][a-z0-9+.-]*:/i.test(value) && !/^https?:/i.test(value)) {
        throw new AuthError("Only http:// and https:// backend URLs are supported.");
    }
    if (!/^https?:\/\//i.test(value)) {
        value = `http://${value}`;
    }

    let url;
    try {
        url = new URL(value);
    } catch {
        throw new AuthError("Enter a valid backend URL.");
    }
    if (url.username || url.password) {
        throw new AuthError("Backend URLs must not contain a user name or password.");
    }
    if (url.search || url.hash) {
        throw new AuthError("Remove the query string and fragment from the backend URL.");
    }
    // Tokens travel to this origin; plaintext is only acceptable on loopback.
    if (url.protocol === "http:" && !isLoopbackHostname(url.hostname)) {
        throw new AuthError("Use https:// for a backend that is not on localhost.");
    }

    let path = url.pathname.replace(/\/+$/, "");
    if (/\/api\/v1$/i.test(path)) {
        path = path.replace(/\/api\/v1$/i, "/api/v1");
    } else {
        path += "/api/v1";
    }
    url.pathname = path;
    return url.toString();
}

function isLoopbackHostname(hostname) {
    const host = hostname.toLowerCase().replace(/\.$/, "");
    return host === "localhost"
        || host.endsWith(".localhost")
        || host === "::1"
        || host === "[::1]"
        || host === "0.0.0.0"
        || /^127(?:\.\d{1,3}){3}$/.test(host);
}

/**
 * Discovers the IdP endpoints. The backend proxies the discovery document
 * because many IdPs do not send CORS headers on it; older backends without
 * the proxy fall back to fetching the issuer directly.
 */
async function discover(apiBase, issuer) {
    try {
        return await getJson(`${apiBase}/oidc-config`, "OIDC discovery");
    } catch (error) {
        if (error.code === "http" && /HTTP 404\b/.test(error.message)) {
            return getJson(discoveryUrl(issuer), "OIDC discovery");
        }
        throw error;
    }
}

function discoveryUrl(issuer) {
    return `${String(issuer).replace(/\/+$/, "")}/.well-known/openid-configuration`;
}

function expiresInMs(data) {
    const seconds = Number(data.expires_in);
    return (Number.isFinite(seconds) && seconds > 0 ? seconds : DEFAULT_EXPIRES_IN) * 1000;
}

function oauthErrorText(data) {
    if (!data) return "no response";
    return data.error_description
        || data.error
        || data.detail
        || data.title
        || data.message
        || "unexpected response";
}

async function getJson(url, label) {
    let response;
    try {
        response = await fetch(url, {
            headers: {Accept: "application/json"},
            signal: timeoutSignal(),
        });
    } catch (e) {
        throw new AuthError(`${label} unreachable: ${url} (${e.message})`, "network");
    }
    if (!response.ok) {
        throw new AuthError(`${label} failed: HTTP ${response.status} (${url})`, "http");
    }
    try {
        return await response.json();
    } catch {
        throw new AuthError(`${label} returned invalid JSON (${url})`, "invalid_json");
    }
}

async function postToken(endpoint, fields) {
    let response;
    try {
        response = await fetch(endpoint, {
            method: "POST",
            headers: {
                "Content-Type": "application/x-www-form-urlencoded",
                Accept: "application/json",
            },
            body: new URLSearchParams(fields),
            signal: timeoutSignal(),
        });
    } catch (e) {
        const viaProxy = endpoint.endsWith("/oauth/token");
        throw new AuthError(
            `Token endpoint unreachable: ${endpoint} (${e.message}).` +
            (viaProxy ? "" : " The provider must allow CORS for this origin."),
            "network",
        );
    }
    const text = await response.text();
    let data = null;
    try {
        data = text ? JSON.parse(text) : {};
    } catch {
        data = {error: text.slice(0, 200)};
    }
    return {status: response.status, data};
}

function timeoutSignal() {
    return typeof AbortSignal !== "undefined" && AbortSignal.timeout
        ? AbortSignal.timeout(HTTP_TIMEOUT_MS)
        : undefined;
}

function randomBase64Url(byteLength) {
    const bytes = new Uint8Array(byteLength);
    crypto.getRandomValues(bytes);
    return bytesToBase64Url(bytes);
}

async function s256(verifier) {
    const digest = await crypto.subtle.digest(
        "SHA-256",
        new TextEncoder().encode(verifier),
    );
    return bytesToBase64Url(new Uint8Array(digest));
}

function bytesToBase64Url(bytes) {
    let binary = "";
    for (const byte of bytes) {
        binary += String.fromCharCode(byte);
    }
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/**
 * Reads one string claim from an ID token without verifying the signature.
 * Only used for the `sub` label and the nonce check: nothing is authorized by
 * the ID token, the access token is.
 */
export function readClaim(idToken, name) {
    if (!idToken) return "";
    try {
        const payload = idToken.split(".")[1];
        const normalized = payload.replace(/-/g, "+").replace(/_/g, "/");
        const padded = normalized + "=".repeat((4 - (normalized.length % 4)) % 4);
        const json = atob(padded);
        const bytes = Uint8Array.from(json, (c) => c.charCodeAt(0));
        const value = JSON.parse(new TextDecoder().decode(bytes))[name];
        return typeof value === "string" ? value : "";
    } catch {
        return "";
    }
}
