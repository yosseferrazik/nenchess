# NenChess

An online chess platform built as a **modular monolith**: one deployable Rust
service with clean internal seams, plus a React SPA. The server is the *only*
authority — the client expresses intentions, never outcomes.

This repository is the **executable skeleton** (roadmap stage 02) with the game
domain, persistence and multiplayer plumbing already in place (stages 03–04):
it compiles, runs, plays real games over WebSocket and is covered by unit and
end-to-end tests.

---

## Table of contents

- [What works today](#what-works-today)
- [Architecture](#architecture)
- [Repository layout](#repository-layout)
- [Quickstart](#quickstart)
- [The move pipeline](#the-move-pipeline)
- [Clocks and results](#clocks-and-results)
- [The contract is Rust](#the-contract-is-rust)
- [Configuration](#configuration)
- [Security and operations](#security-and-operations)
- [Testing](#testing)
- [Decisions](#decisions)
- [Roadmap](#roadmap)
- [Known limitations](#known-limitations)

---

## What works today

- **Identity** — register, login, sessions as `HttpOnly; SameSite=Lax` cookies
  (bearer tokens for API clients), logout with server-side revocation, password
  hashing (argon2id), constant-time verification, no user enumeration.
- **Lobby** — create a game (time control), list open games, join a seat, read
  any game, share an **invite link** (deep link to `/games/:id`, which survives
  a login round-trip).
- **Matchmaking** — enqueue for a time control; a background scheduler pairs two
  waiting players and hands each of them the resulting game id exactly once.
  Rated queues are skill-gated: pairs must fall inside a rating window that
  starts at ±150, widens two points per second anyone in the pair has waited
  and caps at ±1000, nearest rating first; casual queues pair by time control
  alone.
- **Live play over WebSocket** — subscribe, move, resign; server-validated
  legality, checkmate, stalemate and flag fall.
- **Draw offers** — propose, accept, decline or withdraw; answering an offer
  with a counter-offer is mutual agreement. Offers are auxiliary state (no
  version bump, cleared implicitly by a move) and acceptance settles ratings
  and tournaments like any other ending.
- **PGN export & game history** — `GET /api/games/{id}/pgn` replays the
  immutable move records from the starting position, rendering each move as
  SAN through the rules kernel (headers carry both players, the time control
  and the result token); `GET /api/me/games` lists the caller's games newest
  first with colour, opponent and their own outcome, shown on the profile page
  with one-click PGN downloads.
- **Automatic draws** — fifty-move rule, threefold repetition and insufficient
  material, all decided by the server from the authoritative position history.
- **Ratings** — per-category Elo (`bullet`/`blitz`/`rapid`), computed
  server-side when a rated game ends; K-factors 32/16/8 (FIDE-style tiers) and
  a rating history appended in the same transaction as the result.
- **Tournaments** — round-robin groups generated with the circle method (byes
  for odd fields), creator-driven start, pairings that become real `games` rows
  so the authoritative move pipeline applies unchanged, and standings (points →
  Buchholz → rating) settled atomically as games finish.
- **The hard parts, from day one** — optimistic versioning, idempotent
  commands, atomic persistence with a transactional outbox, resumable
  snapshots.
- **Frontend** — auth, lobby with matchmaking and invite links, a board with
  clocks, move list, promotion picker, spectator mode, automatic
  reconnection, profile history with PGN downloads, and tournament
  create/join/start with live standings.

Deliberately **not** built yet: claim-based (rather than automatic) draws,
email, push,
CI deployment. See [Known limitations](#known-limitations).

---

## Architecture

```
                    intent only ─┐
                                 ▼
  React SPA ──HTTP──▶  transport/http  ──▶ modules/*  ──▶ application/*
             ──WS───▶  transport/ws                  ──▶ domain/*
                                                             │
                                        infrastructure/repos │ (SQL)
                                                             ▼
                                                       PostgreSQL
                                                             │
                                          infrastructure/outbox_dispatcher
                                                             │
                                          infrastructure/hub ──▶ WS clients
```

- **Modular monolith.** One process, one database, explicit module boundaries.
  Boundaries are packages and traits, so a module can later be extracted without
  a rewrite.
- **Hexagonal-ish.** `domain` holds the rules and knows nothing about HTTP or
  SQL; `application` orchestrates use cases; `infrastructure` holds adapters
  (Postgres, clock, hub); `transport` is the web layer.
- **PostgreSQL is the source of truth.** Nothing important lives only in a
  process; a restarted server rebuilds live state from the database.

Modules (`services/api/src/modules/`): `identity`, `games`, `matchmaking`,
`ratings`, `tournaments` — all implemented and wired into the router.

## Repository layout

```
apps/web/                 React + TypeScript SPA (Vite)
  src/lib/generated/      ← TypeScript contract types generated from Rust
crates/
  game-core/              Rules + clock (wraps cozy-chess; engine is swappable)
  protocol/               Versioned wire contract (single source of truth)
  test-support/           Shared test factories and board scenarios
services/api/             The server (Axum)
  src/domain/             Game aggregate, results, clock policy
  src/application/        Use cases: the move pipeline, view assembly
  src/infrastructure/     Postgres repos, clock, WS hub, outbox dispatcher
  src/modules/            Bounded contexts
  src/transport/          HTTP and WebSocket interfaces
migrations/               SQL migrations (embedded at compile time)
tests/integration/        End-to-end tests that drive the real server
docs/adr/                 Architecture decision records
infra/docker/             Dockerfiles + nginx config
```

## Quickstart

### Everything with Docker

```bash
docker compose up --build
# Web  → http://localhost:8081  (nginx also proxies the ops endpoints)
# API  → http://localhost:8081/health, /livez, /readyz, /metrics
```

The API container's port is deliberately not published: only nginx reaches it,
which is what makes the trusted-proxy rate-limit keying (below) spoof-proof.

### Local development

Requirements: Rust (stable), Node 24+, and a PostgreSQL 16 instance.

```bash
# 1. Database
docker compose up -d postgres

# 2. Environment
cp .env.example .env          # Linux/macOS
# PowerShell: Copy-Item .env.example .env

# 3. API (runs migrations on startup, then serves 127.0.0.1:8080)
cargo run -p nenchess-api

# 4. Web (proxies /api and /ws to the API, serves http://localhost:5173)
npm install
npm run dev
```

Open <http://localhost:5173>, register two users (in two browsers or one normal
plus one private window), create a game in one, join it from the other.

## The move pipeline

Every state-changing command follows the same path
(`services/api/src/application/moves.rs`):

1. **Lock** the game (in-process `tokio::sync::Mutex` **and** `SELECT … FOR
   UPDATE`), so validation and the write are atomic.
2. **Idempotency** — if the `command_id` was already processed, replay the stored
   response. Retries are therefore safe.
3. **Optimistic concurrency** — reject unless `expected_version` matches the
   current `version`, with `version_conflict`.
4. **Authorization** — the actor must occupy a seat (`forbidden` otherwise).
5. **Domain validation** — `game-core` decides legality, checkmate, stalemate.
6. **Clock** — charge the mover against the server clock, add the increment, or
   declare the flag fallen if the command arrived too late.
7. **Atomic write** — move + state + outbox event in one transaction; `commit`.
8. **Publish** — the outbox dispatcher delivers the event to subscribers
   (at-least-once; clients dedupe by `version`).

Rejections are typed (`error.rs` → `ErrorCode`) and identical between HTTP and
WebSocket, so clients handle failures the same way everywhere.

## Clocks and results

- The server persists `white_ms`, `black_ms`, `increment_ms`, the side to move
  and `turn_started`. Snapshots also carry `server_now_ms` so the UI can
  interpolate smoothly without ever sending time back.
- **Fischer increment**: after a legal move the mover is charged the elapsed
  time and credited the increment.
- **Flag policy (deterministic)**: a move is legal only if it arrives *strictly
  before* `turn_started + remaining`. If it does not, the flag falls, the game
  ends and the late move is **not** recorded — regardless of network jitter.
- Results are computed server-side: `checkmate`, `stalemate`, `flag`,
  `resignation`.

## The contract is Rust

`crates/protocol` is the single source of truth for HTTP and WebSocket payloads.
The TypeScript types used by the SPA are generated from it:

```bash
cargo test -p protocol        # regenerates apps/web/src/lib/generated/*.ts
```

`TS_RS_EXPORT_DIR` in `.cargo/config.toml` points the generator at the frontend.
Generated files are committed so the SPA builds without a Rust toolchain, and CI
typechecks the SPA against the freshly generated contract, so a breaking change
cannot land silently.

## Configuration

All backend settings are environment variables (see `.env.example`):

| Variable | Default | Purpose |
| --- | --- | --- |
| `DATABASE_URL` | *(required)* | PostgreSQL connection string |
| `BIND_ADDR` | `127.0.0.1:8080` | Listen address |
| `DATABASE_MAX_CONNECTIONS` | `10` | Pool size |
| `OUTBOX_POLL_INTERVAL_MS` | `200` | Outbox dispatch cadence |
| `MATCHMAKING_INTERVAL_MS` | `1000` | Matchmaking pairing cadence |
| `SESSION_TTL_DAYS` | `14` | Session lifetime (cookie `Max-Age` too) |
| `RUST_LOG` | `info` | `tracing` filter |
| `RATE_LIMIT_AUTH_PER_MINUTE` | `30` | Register + login per client IP (0 = off) |
| `RATE_LIMIT_API_PER_MINUTE` | `0` | Authenticated API calls per user (0 = off) |
| `RATE_LIMIT_WS_PER_MINUTE` | `120` | WebSocket commands per connection (0 = off) |
| `COOKIE_SECURE` | `false` | Add `Secure` to session cookies (HTTPS) |
| `TRUST_PROXY_HEADERS` | `false` | Trust `X-Real-IP`/`X-Forwarded-For` (behind nginx) |
| `ALLOWED_ORIGINS` | localhost:5173/8081 (and 127.0.0.1) | Extra `Origin` values accepted on cookie CSRF checks (CSV) |

Validation bounds: `initial_time_ms` 1 000–3 600 000, `increment_ms` 0–60 000,
username 3–20 chars `[A-Za-z0-9_]`, password 8–128 chars.

## Security and operations

**Sessions.** Login/registration set an `HttpOnly; SameSite=Lax` session
cookie; the SPA never sees the token (only display data lands in
`localStorage`, revalidated against `/api/auth/me` on page load). API clients
keep using `Authorization: Bearer …`, and the WebSocket upgrade accepts either
`?token=` or the cookie. `POST /api/auth/logout` revokes the session
server-side and clears the cookie.

**CSRF.** Cookie-authenticated unsafe requests must prove same-origin: modern
browsers are held to `Sec-Fetch-Site: same-origin|none`; without it the
`Origin` must be in `ALLOWED_ORIGINS` or match the request `Host`. Bearer
requests are exempt, and `SameSite=Lax` is the outer layer.

**Rate limits.** Fixed windows, per process (zero dependencies): Argon2-backed
register/login per client IP (429 + `Retry-After`), optional authenticated API
budget per user, and a per-WebSocket-connection command budget. Behind nginx
set `TRUST_PROXY_HEADERS=true` — and keep the API port unpublished so headers
can't be spoofed.

**Probes and metrics.**

```bash
curl http://127.0.0.1:8080/livez   # dev: API direct (Docker: http://localhost:8081/livez)
curl http://127.0.0.1:8080/readyz   # 200/503 — DB SELECT 1 under a 2 s budget
curl http://127.0.0.1:8080/metrics  # Prometheus text: HTTP by route/status class,
                                    # WS gauges, moves, outbox, auth, rate limits
curl http://127.0.0.1:8080/health   # legacy combined probe, unchanged
```

Every response carries `x-request-id` (adopted if you send one, generated
otherwise) and each request runs inside a span with the same id, so one id
correlates the proxy log, nginx log and API logs.

## Testing

```bash
# Fast unit tests (rules, clock, domain, hub, contract)
cargo test --workspace

# End-to-end: spawns the real server against a `nenchess_test` database and
# drives it over HTTP and WebSocket (idempotency, version conflicts, races,
# flag fall, resignation, draw offers, PGN export & history, spectators,
# skill-based matchmaking, tournaments, ops probes,
# cookie/CSRF
# lifecycle, rate-limit 429)
cargo test -p nenchess-integration --test e2e -- --nocapture

npm run lint && npm run typecheck && npm run build

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The end-to-end suite needs a reachable PostgreSQL; override the connection with
`NENCHESS_TEST_DATABASE_URL` (defaults to the compose instance). It creates and
migrates its own database, so it never touches development data.

## Decisions

Architecture decision records live in [`docs/adr/`](docs/adr/):

- **0001** — Use `cozy-chess` (MIT), not `shakmaty` (GPL-3.0-or-later), and hide
  the engine behind `game-core` so the choice stays reversible.
- **0002** — Authoritative server: optimistic concurrency, idempotent commands,
  and a transactional outbox for at-least-once delivery.
- **0003** — Platform growth: per-category Elo settled with the game, a single
  scheduler for matchmaking, and round-robin tournaments reusing the game
  engine.
- **0004** — Hardening: `HttpOnly` cookie sessions with layered CSRF checks,
  dependency-free fixed-window rate limits, and probes/Prometheus metrics with
  request-id correlation.

## Roadmap

1. **Decisions & ADRs** — ✅ engine licensing, protocol, concurrency model.
2. **Executable skeleton** — ✅ workspace, services, migrations, CI, Docker.
3. **Game domain & persistence** — ✅ move pipeline, clocks, results, atomicity.
4. **Multiplayer & resilience** — ✅ WebSocket hub, reconnection, snapshots; ⏳
   multi-replica fan-out.
5. **Platform growth** — ✅ ratings, tournaments, skill-based matchmaking,
   draw offers with automatic draw detection, PGN export & game history.

## Known limitations

- **Single replica.** The WS hub is in-process, the per-game lock is a `tokio`
  mutex and matchmaking runs a single scheduler in-process; rate limiters and
  metrics are per-process too, so replicas multiply the request budgets and
  fragment `/metrics`. Running multiple API instances requires swapping the
  publisher for a shared broker and the lock for a database/broker-level one;
  the pairing SQL already uses `FOR UPDATE SKIP LOCKED`, so the scheduler
  itself is safe to run in parallel. The outbox contract does not change.
- **Draws by rule are automatic, not claimed.** Threefold repetition and
  insufficient material end the game as soon as the server detects them; FIDE
  would let the player claim instead. Fifty-move and stalemate are also
  automatic. Players can also agree to a draw (`game.draw_offer`), which does
  follow the standard offer/accept flow.
- **Engine coverage** — `cozy-chess` covers standard chess only; variants are
  out of scope for now.

---

Licensed under a license to be decided. Note that `cozy-chess` is MIT; no
GPL-licensed component is linked (see ADR 0001).
