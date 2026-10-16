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
- [Testing](#testing)
- [Decisions](#decisions)
- [Roadmap](#roadmap)
- [Known limitations](#known-limitations)

---

## What works today

- **Identity** — register, login, bearer sessions, password hashing (argon2id),
  constant-time verification, no user enumeration.
- **Lobby** — create a game (time control), list open games, join a seat, read
  any game.
- **Live play over WebSocket** — subscribe, move, resign; server-validated
  legality, checkmate, stalemate and flag fall.
- **The hard parts, from day one** — optimistic versioning, idempotent
  commands, atomic persistence with a transactional outbox, resumable
  snapshots.
- **Frontend** — auth, lobby, a board with clocks, move list, promotion picker,
  spectator mode and automatic reconnection.

Deliberately **not** built yet: ratings/ELO calculation, matchmaking queue,
tournaments, draw by repetition/insufficient material, PGN/SAN export, email,
push, CI deployment. See [Known limitations](#known-limitations).

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
`ratings`, `tournaments`. Only `identity` and `games` are implemented; the rest
carry the intended contract and are wired into the router as they arrive.

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
# API  → http://localhost:8080/health
# Web  → http://localhost:8081
```

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
| `SESSION_TTL_DAYS` | `14` | Session lifetime |
| `RUST_LOG` | `info` | `tracing` filter |

Validation bounds: `initial_time_ms` 1 000–3 600 000, `increment_ms` 0–60 000,
username 3–20 chars `[A-Za-z0-9_]`, password 8–128 chars.

## Testing

```bash
# Fast unit tests (rules, clock, domain, hub, contract)
cargo test --workspace

# End-to-end: spawns the real server against a `nenchess_test` database and
# drives it over HTTP and WebSocket (idempotency, version conflicts, races,
# flag fall, resignation, spectators)
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

## Roadmap

1. **Decisions & ADRs** — ✅ engine licensing, protocol, concurrency model.
2. **Executable skeleton** — ✅ workspace, services, migrations, CI, Docker.
3. **Game domain & persistence** — ✅ move pipeline, clocks, results, atomicity.
4. **Multiplayer & resilience** — ✅ WebSocket hub, reconnection, snapshots; ⏳
   multi-replica fan-out.
5. **Platform growth** — ⏳ ratings, matchmaking, tournaments, draw detection.

## Known limitations

- **Single replica.** The WS hub is in-process and the per-game lock is a
  `tokio` mutex. Running multiple API instances requires swapping the publisher
  for a shared broker and the lock for a database/broker-level one; the outbox
  contract does not change.
- **No draw by repetition / insufficient material** yet (explicit TODOs in
  `domain/game.rs`); fifty-move and stalemate are handled.
- **Bearer tokens in `localStorage`.** Fine for the skeleton; httpOnly cookies
  are the intended hardening step.
- **No ratings yet** — the `ratings` module and tables are reserved but not
  populated.
- **Engine coverage** — `cozy-chess` covers standard chess only; variants are
  out of scope for now.

---

Licensed under a license to be decided. Note that `cozy-chess` is MIT; no
GPL-licensed component is linked (see ADR 0001).
