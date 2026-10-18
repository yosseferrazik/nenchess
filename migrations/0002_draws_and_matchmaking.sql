-- Stage 05 additions: automatic draws (threefold repetition, insufficient
-- material) and skill-based matchmaking.
--
-- Design notes:
--   * `games.position_hashes` stores the Zobrist hash of every position the
--     game has passed through, starting with the initial position. It is
--     appended on every accepted move, which makes threefold-repetition
--     detection O(n) in the number of moves without replaying the game.
--   * `matchmaking_queue` is a small, in-process-scheduler driven queue. Rows
--     carry the resulting `matched_game_id` so a polling client can be handed
--     its game even after the row leaves the "unmatched" set.

-- ---------------------------------------------------------------------------
-- Draw detection
-- ---------------------------------------------------------------------------

ALTER TABLE games
    ADD COLUMN position_hashes bigint[] NOT NULL DEFAULT ARRAY[]::bigint[];

-- Extend the accepted result reasons with the two new automatic draws.
ALTER TABLE games DROP CONSTRAINT games_result_reason_check;
ALTER TABLE games ADD CONSTRAINT games_result_reason_check CHECK (
    result_reason IN (
        'checkmate', 'stalemate', 'resignation', 'flag', 'fifty_move',
        'threefold_repetition', 'insufficient_material', 'abandonment'
    )
);

-- ---------------------------------------------------------------------------
-- Matchmaking
-- ---------------------------------------------------------------------------

CREATE TABLE matchmaking_queue (
    user_id uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    initial_time_ms bigint NOT NULL,
    increment_ms bigint NOT NULL,
    rated boolean NOT NULL DEFAULT false,
    enqueued_at timestamptz NOT NULL DEFAULT now(),
    -- Set by the pairing scheduler; the client consumes and clears it by
    -- fetching its matchmaking status.
    matched_game_id uuid REFERENCES games (id) ON DELETE SET NULL
);

-- The scheduler scans for pairable entries by time control; the partial index
-- keeps that scan over the (small) unmatched set only.
CREATE INDEX matchmaking_queue_open_idx
    ON matchmaking_queue (initial_time_ms, increment_ms, rated, enqueued_at)
    WHERE matched_game_id IS NULL;
