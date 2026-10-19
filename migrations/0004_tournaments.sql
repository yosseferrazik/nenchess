-- Tournaments: simple round-robin (Swiss-like minimal set for stage 05).

BEGIN;

CREATE TABLE tournaments (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    status text NOT NULL DEFAULT 'registration'
        CHECK (status IN ('registration', 'active', 'finished')),
    time_control_initial_ms bigint NOT NULL,
    time_control_increment_ms bigint NOT NULL,
    rated boolean NOT NULL DEFAULT true,
    max_participants integer NOT NULL DEFAULT 16 CHECK (max_participants > 0),
    current_round integer NOT NULL DEFAULT 0 CHECK (current_round >= 0),
    total_rounds integer NOT NULL DEFAULT 0 CHECK (total_rounds >= 0),
    created_by uuid NOT NULL REFERENCES users (id),
    started_at timestamptz,
    finished_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tournament_participants (
    tournament_id uuid NOT NULL REFERENCES tournaments (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    rating_snapshot integer NOT NULL DEFAULT 1500,
    points double precision NOT NULL DEFAULT 0.0 CHECK (points >= 0.0),
    games_played integer NOT NULL DEFAULT 0,
    buchholz integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tournament_id, user_id)
);

CREATE TABLE tournament_rounds (
    id uuid PRIMARY KEY,
    tournament_id uuid NOT NULL REFERENCES tournaments (id) ON DELETE CASCADE,
    round_number integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    UNIQUE (tournament_id, round_number)
);

CREATE TABLE tournament_pairings (
    id uuid PRIMARY KEY,
    tournament_id uuid NOT NULL REFERENCES tournaments (id) ON DELETE CASCADE,
    round_id uuid NOT NULL REFERENCES tournament_rounds (id) ON DELETE CASCADE,
    round_number integer NOT NULL,
    white_id uuid REFERENCES users (id),
    black_id uuid REFERENCES users (id),
    game_id uuid REFERENCES games (id),
    bye boolean NOT NULL DEFAULT false,
    result text CHECK (result IN ('white_win', 'black_win', 'draw', 'bye')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (bye OR white_id IS NOT NULL OR black_id IS NOT NULL)
);

CREATE INDEX tournaments_status_created_idx ON tournaments (status, created_at DESC);
CREATE INDEX tournament_participants_tournament_idx ON tournament_participants (tournament_id);
CREATE INDEX tournament_pairings_tournament_idx ON tournament_pairings (tournament_id, round_number);
CREATE INDEX tournament_pairings_game_idx ON tournament_pairings (game_id);

COMMIT;
