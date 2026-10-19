-- Stage 06a: draw offers by agreement.
--
-- Design notes:
--   * The pending offer is auxiliary state. It never bumps `games.version`
--     (clients merge it through its dedicated `game.draw_offer_changed`
--     event), and a legal move clears it inside the same transaction that
--     records the move.
--   * Accepting settles the game through the exact same path as resignation:
--     ratings and tournament pairings update atomically with the result.

ALTER TABLE games
    ADD COLUMN draw_offered_by text CHECK (draw_offered_by IN ('white', 'black'));

-- Draw by agreement joins the result reasons.
ALTER TABLE games DROP CONSTRAINT games_result_reason_check;
ALTER TABLE games ADD CONSTRAINT games_result_reason_check CHECK (
    result_reason IN (
        'checkmate', 'stalemate', 'resignation', 'flag', 'fifty_move',
        'threefold_repetition', 'insufficient_material', 'abandonment',
        'agreement'
    )
);
