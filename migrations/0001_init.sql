-- NenChess initial schema.
--
-- Principles:
--   * PostgreSQL is the source of truth; nothing here is reconstructible-only.
--   * Moves are immutable records; the current state is a snapshot (`games`).
--   * Concurrency is guarded by `games.version` (optimistic) plus row locks.
--   * Events destined to other components go through `outbox_events` in the
--     same transaction as the state change.
--   * Commands are deduplicated through `idempotency_keys`.

-- ---------------------------------------------------------------------------
-- Identity
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    id uuid PRIMARY KEY,
    username text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_username_lower_key ON users (lower(username));

CREATE TABLE user_credentials (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    password_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    token text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);

CREATE TABLE profiles (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    display_name text
);

-- ---------------------------------------------------------------------------
-- Games
-- ---------------------------------------------------------------------------

CREATE TABLE games (
    id uuid PRIMARY KEY,
    white_player_id uuid NOT NULL REFERENCES users (id),
    black_player_id uuid REFERENCES users (id),
    status text NOT NULL DEFAULT 'waiting'
        CHECK (status IN ('waiting', 'active', 'finished')),
    fen text NOT NULL,
    side_to_move text NOT NULL CHECK (side_to_move IN ('white', 'black')),
    -- Optimistic concurrency: every accepted transition increments this.
    version bigint NOT NULL DEFAULT 0,
    move_count integer NOT NULL DEFAULT 0,
    initial_time_ms bigint NOT NULL,
    increment_ms bigint NOT NULL,
    rated boolean NOT NULL DEFAULT false,
    -- Clock snapshot: remaining time per side as of their last turn start,
    -- plus the reference instant of the running turn.
    white_time_ms bigint NOT NULL,
    black_time_ms bigint NOT NULL,
    turn_started_at timestamptz NOT NULL,
    result_winner text CHECK (result_winner IN ('white', 'black')),
    result_reason text CHECK (
        result_reason IN (
            'checkmate', 'stalemate', 'resignation', 'flag', 'fifty_move', 'abandonment'
        )
    ),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX games_status_created_idx ON games (status, created_at DESC);

-- Immutable move history: enough to replay the game move by move.
CREATE TABLE game_moves (
    game_id uuid NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    move_number integer NOT NULL,
    from_sq text NOT NULL,
    to_sq text NOT NULL,
    promotion text,
    uci text NOT NULL,
    fen_after text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (game_id, move_number)
);

-- ---------------------------------------------------------------------------
-- Competition (initial slice; tournaments arrive with stage 05)
-- ---------------------------------------------------------------------------

CREATE TABLE ratings (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    rating integer NOT NULL DEFAULT 1200,
    games_played integer NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE rating_history (
    id bigserial PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    game_id uuid REFERENCES games (id) ON DELETE SET NULL,
    rating_before integer NOT NULL,
    rating_after integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX rating_history_user_idx ON rating_history (user_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- Business infrastructure
-- ---------------------------------------------------------------------------

CREATE TABLE outbox_events (
    id bigserial PRIMARY KEY,
    aggregate_type text NOT NULL,
    aggregate_id uuid NOT NULL,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    dispatched_at timestamptz
);

-- Partial index over the pending tail, which is the only part ever scanned.
CREATE INDEX outbox_events_pending_idx ON outbox_events (id) WHERE dispatched_at IS NULL;

CREATE TABLE idempotency_keys (
    command_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind text NOT NULL,
    response jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE audit_events (
    id bigserial PRIMARY KEY,
    user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    kind text NOT NULL,
    detail jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_kind_idx ON audit_events (kind, created_at DESC);
