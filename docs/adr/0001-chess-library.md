# ADR 0001 — Chess rules engine: `cozy-chess` instead of `shakmaty`

- **Status:** accepted
- **Date:** 2026-10-16
- **Deciders:** platform team
- **Context stage:** 01 (decisions) / 03 (game domain)

## Context

The server must be the single authority on chess legality, check/checkmate,
stalemate and move generation. Hand-rolling the rules is not acceptable:
the surface for bugs (en passant, castling rights, promotion, repetition,
fifty-move, insufficient material) is enormous and correctness is
non-negotiable.

The obvious candidate, `shakmaty`, is the best-known Rust chess library, but it
is licensed **GPL-3.0-or-later**. Linking it into the server would make the
combined work subject to the GPL, which is incompatible with the intended
direction of the product (a closed/hosted service without a licensing decision
yet). Adopting it *before* that decision would effectively decide the licence of
the whole backend.

## Decision

Use **`cozy-chess` 0.3 (MIT)** as the rules engine, and hide it completely
behind `crates/game-core`'s own API:

- `game-core` exposes `starting_fen`, `validate_and_apply`, `status`,
  `MoveInput`, `Color`, `PieceKind`, `Square` and its own `GameStatus`.
- No `cozy-chess` type appears in `crates/protocol`, in the API layer, or in the
  database schema (which stores plain FEN strings and UCI moves).
- The engine is therefore swappable: replacing it means changing one crate, and
  the wire contract, persistence and frontend stay untouched.

`shakmaty` remains a viable future option *if and only if* a GPL-compatible
licensing decision is made for the product; the abstraction above keeps that
door open at low cost.

## Consequences

**Positive**
- MIT-compatible dependency: no licence contamination, decision left open.
- Engine isolation makes the "no hand-rolled rules, but swappable engine" claim
  real rather than aspirational.
- FEN/UCI on the wire means clients and tooling are standard.

**Negative / accepted trade-offs**
- `cozy-chess` has a smaller ecosystem than `shakmaty`; certain high-level
  helpers (e.g. rich SAN, PGN, opening book support) must be built or added
  later.
- Some advanced draw detection is not wired yet: fifty-move and stalemate are
  handled, while **threefold repetition and insufficient material are explicit
  TODOs** in `services/api/src/domain/game.rs`.
- The abstraction layer is code we maintain (thin, but non-zero).

## Alternatives considered

1. **`shakmaty` (GPL-3.0-or-later)** — rejected for now on licensing grounds;
   technically excellent.
2. **Hand-written rules** — rejected: unacceptable correctness risk and effort.
3. **`chess` / `pgn-reader` and similar crates** — smaller/less maintained, and
   they do not solve the licensing question any better than an MIT engine while
   offering less (or less clear) correctness evidence.

## Follow-up

- Add threefold-repetition and insufficient-material detection.
- Add SAN/PGN rendering for display and export (derived from UCI, derived
  server-side so the client stays rules-free).
