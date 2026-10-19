# ADR 0004 — Hardening: cookie sessions with CSRF, layered rate limits, first-class observability

- **Status:** accepted
- **Date:** 2026-10-19
- **Deciders:** platform team
- **Context stage:** 06 (hardening)

## Context

Until now the SPA held the session token in `localStorage` and attached it as
`Authorization: Bearer …` (WebSocket: `?token=`). That works, but it means any
XSS sink can exfiltrate a live session, and tokens leak into URLs, proxy logs
and browser history. Alongside authentication, three operational gaps remain:

1. Register/login runs Argon2 and is the only expensive, unauthenticated
   endpoint — a natural credential-stuffing and CPU-exhaustion target with no
   request budget at all.
2. There is no way to distinguish "process alive" from "database reachable" for
   orchestrators, and no metrics to alert on.
3. Failures are hard to correlate: a request has no id that spans log lines.

Constraints carried over: the server stays the only authority, the workspace
keeps its auditable dependency tree (no new crates for any of this), and
`crates/protocol` remains the single wire-contract source (no contract changes
were needed).

## Decision

### 1. Sessions: `HttpOnly` cookie for browsers, bearer for API clients

- Login/registration set `nenchess_session=<token>; HttpOnly; SameSite=Lax;
  Path=/; Max-Age=<ttl>[; Secure]`. The token never reaches JavaScript; the SPA
  stores only display data (username, expiry) in `localStorage` and revalidates
  against `GET /api/auth/me` on every page load. `COOKIE_SECURE=true` adds
  `Secure` behind HTTPS.
- `require_auth` accepts **bearer first, then cookie**; the WebSocket upgrade
  accepts `?token=` (API clients, e2e) or the cookie from the handshake headers,
  so browsers no longer put tokens in URLs. `POST /api/auth/logout` is new,
  idempotent: it deletes the session row (server-side revocation) and clears the
  cookie; sessions remain server-side records with an expiry, as before.
- **CSRF, defense-in-depth over `SameSite=Lax`, applied only to
  cookie-authenticated unsafe methods** (bearer requests are immune — no cookie
  rides along):
  1. if `Sec-Fetch-Site` is present (every modern browser): only `same-origin`
     and `none` pass — `same-site` and `cross-site` are rejected;
  2. else if `Origin` is present: it must match `ALLOWED_ORIGINS` or the
     request's `Host` authority (hostname comparison survives nginx's
     `Host $host`, which strips the port);
  3. else (no browser headers): allowed — such clients should be using bearer
     auth anyway.
- The Vite dev proxy runs with `changeOrigin: false` so the browser's `Host`
  reaches the API unchanged in development.

### 2. Rate limits: fixed windows, three scopes, zero dependencies

- A hand-rolled fixed-window `RateLimiter` (`infrastructure/rate_limit.rs`,
  `Mutex<HashMap<key, Bucket>>`, amortized cleanup, unit-tested) behind three
  config knobs (0 = disabled):
  - `RATE_LIMIT_AUTH_PER_MINUTE` (default 30) — register + login, keyed by
    client IP, enforced **before** any Argon2 work. Rejections are `429` with a
    `Retry-After` header and `ErrorCode::RateLimited`.
  - `RATE_LIMIT_API_PER_MINUTE` (default 0/off) — authenticated API calls per
    user, checked inside `require_auth` so the key is the authenticated user.
  - `RATE_LIMIT_WS_PER_MINUTE` (default 120) — commands per WebSocket
    connection (counted locally in the socket loop, including malformed ones);
    exceeding it sends an `error` frame instead of closing.
- Client identity: `ConnectInfo<SocketAddr>` by default; when
  `TRUST_PROXY_HEADERS=true`, `X-Real-IP` first (nginx overwrites it — not
  client-spoofable), then the **last** `X-Forwarded-For` hop (the one appended
  by our proxy; any client-supplied prefix is attacker controlled). Because a
  spoofed `X-Real-IP` would defeat this, the compose deployment does **not**
  publish the API port — only nginx reaches it.
- This is a soft, per-process protection layer (see trade-offs), not a
  cluster-wide quota.

### 3. Observability: probes, Prometheus metrics, request ids

- `GET /livez` (process liveness, never touches the DB), `GET /readyz`
  (DB `SELECT 1` under a 2 s timeout → 200/503), `GET /metrics` (Prometheus
  text) — `/health` is kept unchanged for compatibility. nginx passes all four
  through, since the API port is not published.
- `Metrics` (`infrastructure/metrics.rs`) is a hand-rolled counter registry:
  HTTP requests by `(method, MatchedPath template, status class)` — bounded
  cardinality, raw paths never become labels — plus cumulative durations, and
  single-name counters/gauges for WS connections, committed moves, games
  created, outbox events, matchmaking pairs, auth logins/failures and rate-limit
  rejections. Rendering the text format is deterministic (sorted) and
  unit-tested.
- `x-request-id`: adopted when sent (validated: ≤ 64 ASCII alnum/`-`/`_`/`.`),
  generated (UUID) otherwise; attached to the response, to request extensions
  and to an `info_span!("request", …)` so every log line of one request shares
  one id. nginx forwards its `$request_id` as the canonical edge id.

## Consequences

**Positive**
- XSS alone no longer yields a reusable session token; logout revokes
  server-side for every transport (REST and WS).
- Credential stuffing gets a typed, observable429 with `Retry-After` before any
  hashing cost; a single misbehaving WebSocket client cannot saturate the
  validation path.
- Orchestrators get real liveness/readiness split; dashboards and alerts can be
  built on `/metrics`; incidents are traceable end-to-end via request ids.
- All of it ships with **zero new dependencies**; the e2e suite asserts the
  cookie/CSRF/logout lifecycle, the probes, header echo and an actual 429
  (hammered last, deterministically).

**Negative / accepted trade-offs**
- Rate limiters and metrics are in-process (fixed window, no histograms) — one
  replica's counters are that replica's; multiple replicas multiply the budget
  and fragment metrics. Same blast radius as the in-process hub/lock/scheduler
  (see ADR 0003 and Known limitations).
- The `Origin` fallback compares authorities, not full origins: two services on
  different ports of the same host would accept each other's `Origin` for
  browsers that send neither `Sec-Fetch-Site` nor appear in `ALLOWED_ORIGINS`.
  Modern browsers always send fetch metadata, and `SameSite=Lax` still blocks
  the cookie regardless.
- `SameSite=Lax` intentionally allows the cookie on top-level GET navigations;
  those are safe methods and CSRF-checked ones are the unsafe ones.

## Future directions (deliberately deferred)

- Cluster-wide limits (nginx `limit_req` in front, or a shared store behind
  `RateLimiter` — call sites only depend on `check`).
- Request-duration histograms and per-route latency budgets.
- Short-lived single-use tickets for WebSocket URLs (cookie covers first-party
  browsers today).
- Mandatory TLS/`Secure` cookies end-to-end (deployment concern: put the whole
  stack behind HTTPS).
