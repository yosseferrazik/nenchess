# ADR 0002 — Authoritative server: optimistic concurrency, idempotency and a transactional outbox

- **Status:** accepted
- **Date:** 2026-10-16
- **Deciders:** platform team
- **Context stage:** 01 (decisions) / 03 (game domain) / 04 (multiplayer & resilience)

## Context

Chess is a strictly ordered, turn-based domain where a duplicated move, a lost
move or a clock computed on the client corrupts the game. The client may
retry after a dropped connection, two clients may act at the same instant, and
a process may crash between "state written" and "clients notified". We need
rules that make every one of those cases boring.

Four failure modes drive the design:

1. **Lost update** — two intents validated against the same state, last write
   wins silently.
2. **Duplicate intent** — a retried command applied twice.
3. **Torn write** — the move persisted, the notification not (or vice versa).
4. **Untrusted clocks / results** — a client claiming time or a result.

## Decision

### 1. The client only sends intentions; the server computes everything

Commands carry `command_id`, `game_id` and `expected_version`; they never carry
time deltas or results. The server validates legality (`game-core`), charges
the clock, decides checkmate/stalemate/flag/resignation, and publishes the new
state. The clocks are persisted (`white_ms`, `black_ms`, `turn`, `turn_started`)
and elapsed time is derived from the **server** clock only; a move is applied
only if it arrives strictly before `turn_started + remaining`, otherwise the
flag falls and ends the game deterministically.

### 2. Optimistic concurrency + a per-game lock

Every state change bumps a monotonic `version`. A command states the version it
was computed against; a mismatch is rejected with `version_conflict`. On the
server this is enforced by a `SELECT ... FOR UPDATE` row lock plus an in-process
per-game `tokio::sync::Mutex`, so validation and the version bump are atomic
and the loser of a race is told *why* instead of being silently dropped.

### 3. Idempotency by `command_id`

The response produced by a command is stored under its `command_id`. Re-sending
the same command replays the stored response instead of applying the effect
twice. This is what makes a client "retry until answered" safe.

### 4. Atomic persistence via a transactional outbox

Move + resulting state + the outbound event are written in **one transaction**.
The event goes to `outbox_events`; a dispatcher polls and publishes to the
in-process WebSocket hub. Delivery is therefore **at-least-once**, and clients
dedupe by `version` (never apply an older snapshot). A crash can lose a
notification but can never leave the database inconsistent, and the event is
re-dispatched.

### 5. Bearer sessions, not cookies (for the skeleton)

`POST /api/auth/login` mints an opaque token stored in `sessions`; the client
sends it as `Authorization: Bearer`, and the WebSocket authenticates via a
query parameter. This keeps the API trivially usable by the SPA, integration
tests and future non-browser clients. (Moving to httpOnly cookies for the
browser is a later hardening step, not a design change.)

## Consequences

**Positive**
- Deterministic outcomes under concurrency, retries and crashes.
- The same guarantees extend to tournaments by reusing the game engine.
- The rules are testable end-to-end: `tests/integration/tests/e2e.rs` asserts
  duplicate-command replay, stale-version conflict, illegal move, concurrent
  move race, flag expiry, resignation and spectator rejection.

**Negative / accepted trade-offs**
- Clients must carry `command_id`/`expected_version` and tolerate duplicate
  snapshots (a few lines of code, demonstrated in `apps/web/src/lib/ws.ts`).
- The outbox adds a table, an index and a polling loop.
- At-least-once delivery means "exactly once" is realised by version-based
  deduplication, not by the transport.

## Future directions (deliberately deferred)

- **Post-commit publication fan-out**: today the hub is in-process. To run more
  than one API replica, swap the publisher for a shared broker (e.g. Postgres
  `LISTEN/NOTIFY` or NATS) while keeping the outbox contract identical.
- **Multi-replica locks**: the per-game `tokio` mutex becomes advisory-lock or
  broker-based; `FOR UPDATE` already serialises at the database level.
- **Ratings**: computed from finished games in the `ratings` module (stage 05),
  never from client-supplied numbers.
