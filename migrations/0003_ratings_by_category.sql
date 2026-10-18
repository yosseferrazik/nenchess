-- Per-category Elo ratings.
--
-- The skeleton's single-row `ratings` table was reserved but never populated;
-- replace it with per-(user, category) rows so bullet/blitz/rapid ladders are
-- independent.

DROP TABLE IF EXISTS ratings;

CREATE TABLE player_ratings (
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    category text NOT NULL CHECK (category IN ('bullet', 'blitz', 'rapid')),
    rating integer NOT NULL DEFAULT 1500 CHECK (rating >= 0),
    games_played integer NOT NULL DEFAULT 0 CHECK (games_played >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, category)
);

-- Seed the standard categories for users that already exist. New users are
-- seeded on registration (and on demand, defensively).
INSERT INTO player_ratings (user_id, category)
SELECT id, category
FROM users
CROSS JOIN (VALUES ('bullet'), ('blitz'), ('rapid')) AS categories (category)
ON CONFLICT (user_id, category) DO NOTHING;
