# ADR 0003 — Platform growth: Elo ratings, server-side matchmaking and tournaments

- **Status:** accepted
- **Date:** 2026-10-19
- **Deciders:** platform team
- **Context stage:** 05 (platform growth)

## Context

Stage 05 adds the features that turn a working chess server into a platform:
players want to be ranked, to be paired with someone of similar strength, and
to compete in groups. Three constraints carry over from ADR 0002:

1. The server is the only authority — clients must never supply a rating
   change, an opponent or a game result.
2. Every consequence of a finished game must be atomic with the result itself;
   a crash can never leave ratings or standings half-updated.
3. The game engine is reused, never duplicated: tournaments must run through
   the same move pipeline (clocks, legality, outbox, idempotency).

## Decision

### 1. Ratings: per-category Elo, computed only from finished games

- Each player has three independent ratings — `bullet`, `blitz`, `rapid` —
  defaulting to 1500 on first use. The category is derived **server-side** from
  the game's initial time control (< 3 min → bullet, < 10 min → blitz, ≥ 10
  min → rapid).
- K-factor follows FIDE-style tiers: 32 for the first 30 games or ratings
  below 1800 (provisional), 16 up to 2400, 8 beyond (master).
- `ratings::settle` runs inside the same transaction that persists the game
  result: the `player_ratings` row and the `rating_history` before/after rows
  commit or roll back together with the result. There is no path that records a
  result without rating it.
- Clients receive ratings read-only through `GET /api/profile`; no endpoint
  accepts a rating as input.

### 2. Matchmaking: one scheduler, oldest-first, `SKIP LOCKED`

- Players enqueue a `(user, time control)` entry; they never choose an
  opponent. A single background scheduler pairs entries oldest-first, so no
  HTTP request can create a game by itself.
- The queue query uses `FOR UPDATE SKIP LOCKED`, which keeps the scheduler safe
  to run in parallel instances later (the same pattern as the outbox).
- The game and the `matched_game_id` of both entries are written in one
  transaction: a client always polls to a committed game, and re-entering the
  queue replaces the previous match exactly once.
- Rating is deliberately **not** part of pairing yet (first-come-first-served);
  skill-based search is a known limitation, not an accident.

### 3. Tournaments: round-robin groups over the authoritative engine

- Format: a single **round-robin** per tournament, schedule generated with the
  circle method. Odd fields are padded with a virtual bye slot, giving `n`
  rounds for `n` players (odd) or `n-1` (even), exactly one bye per round, and
  colours alternating across rounds.
- Lifecycle: `registration` → `active` → `finished`. Only the creator can
  start, and only with ≥ 2 participants. Starting freezes the roster: every
  pairing is persisted up front, each non-bye pairing gets a real `games` row
  for its round, and byes score their point immediately.
- **Settlement is a hook on game end, never a client report.** When a tournament
  game finishes (move, flag or resign — the three paths that end a game),
  `tournaments::settle_game` runs in the same transaction: it records the
  pairing result idempotently, awards points (win 1, draw 0.5, bye 1), and once
  every pairing of the current round is decided it either opens the next round
  (creating its games) or finishes the tournament.
- Standings order: points, then **Buchholz** (sum of opponents' points), then
  the entry's rating snapshot, then username — fully deterministic.
- Participants carry a `rating_snapshot` taken at registration, so seeding and
  tie-breaks are stable even if live ratings move during the event.

## Consequences

**Positive**
- Ratings and standings cannot drift from game results: they are derived
  data updated atomically with them.
- Tournament games inherit every guarantee of ADR 0002 (idempotency, versioning,
  outbox, flag fall) for free — the engine is called, not copied.
- The e2e suite exercises the whole lifecycle (create → join → start → play all
  rounds → finish → ordered standings) through the public API.

**Negative / accepted trade-offs**
- Round-robin grows quadratically: a 64-player group schedules 2016 games.
  Swiss/knockout formats are future work.
- Buchholz is recomputed from current pairings at read time instead of being
  stored incrementally (read cost is one scan of the group's pairings).
- Standings refresh on the client by polling (4 s) while a tournament is
  active; there is no tournament-specific WebSocket event yet.
- Matchmaking still ignores rating.

## Future directions (deliberately deferred)

- Skill-based matchmaking (Elo windows / expanding search ranges).
- Swiss and knockout formats; scheduled start times for tournaments.
- Claim-based draws and draw offers between players (today draws are
  automatic only).
- Tournament standings pushed over WebSocket instead of polled.
