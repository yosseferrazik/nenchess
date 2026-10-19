-- Swiss tournament format.
--
-- Round-robin stays the default: a single group plays everyone else once.
-- Swiss plays a fixed number of rounds -- ceil(log2(n)) -- pairing players on
-- similar scores each round. The next round's pairings only exist once the
-- previous one is fully decided, so they are generated lazily while the
-- tournament row is locked (exactly once) instead of upfront like the
-- round-robin circle schedule.

ALTER TABLE tournaments
    ADD COLUMN format text NOT NULL DEFAULT 'round_robin'
        CHECK (format IN ('round_robin', 'swiss'));
