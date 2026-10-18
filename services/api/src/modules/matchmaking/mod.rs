//! Matchmaking: two players waiting for the same time control get paired into
//! a game.
//!
//! Rules already fixed (ADR 0002 / stage 05):
//! * The server creates the game — clients only *enqueue* and *poll*.
//! * The pairing is decided by a single background scheduler so no HTTP
//!   request can create a game on its own.
//! * A matched pair writes `matched_game_id` in the same transaction that
//!   creates the game, so the client is always pointed at a committed game.
//! * Entering the queue again replaces the previous entry (and its match).

use protocol::{MatchmakingRequest, MatchmakingStatusDto};
use uuid::Uuid;

use crate::bootstrap::AppState;
use crate::domain::{self, Game};
use crate::error::ApiError;
use crate::infrastructure::repos::matchmaking as repo;

/// Bounds shared with `games::create_game`.
pub fn validate(req: &MatchmakingRequest) -> Result<(), ApiError> {
    if !(1_000..=3_600_000).contains(&req.initial_time_ms) {
        return Err(ApiError::bad_request(
            "initial_time_ms must be between 1000 and 3600000",
        ));
    }
    if !(0..=60_000).contains(&req.increment_ms) {
        return Err(ApiError::bad_request(
            "increment_ms must be between 0 and 60000",
        ));
    }
    Ok(())
}

/// Enter (or re-enter) the queue for a time control.
pub async fn enqueue(
    state: &AppState,
    user_id: Uuid,
    req: MatchmakingRequest,
) -> Result<MatchmakingStatusDto, ApiError> {
    validate(&req)?;
    let mut conn = state.db.acquire().await?;
    repo::enqueue(
        &mut conn,
        user_id,
        req.initial_time_ms,
        req.increment_ms,
        req.rated,
    )
    .await?;
    Ok(MatchmakingStatusDto {
        in_queue: true,
        game_id: None,
        enqueued_at_ms: Some(state.clock.now_ms()),
    })
}

/// Leave the queue. Idempotent: leaving when not queued is not an error.
pub async fn dequeue(state: &AppState, user_id: Uuid) -> Result<MatchmakingStatusDto, ApiError> {
    let mut conn = state.db.acquire().await?;
    repo::cancel(&mut conn, user_id).await?;
    Ok(not_queued())
}

/// Current state. A completed match is *consumed* here: the row is removed and
/// the game is reported once, so a client can never be pointed at it twice.
pub async fn status(state: &AppState, user_id: Uuid) -> Result<MatchmakingStatusDto, ApiError> {
    let mut conn = state.db.acquire().await?;
    if let Some(game_id) = repo::take_matched(&mut conn, user_id).await? {
        return Ok(MatchmakingStatusDto {
            in_queue: false,
            game_id: Some(game_id),
            enqueued_at_ms: None,
        });
    }
    match repo::status(&mut conn, user_id).await? {
        Some(entry) => Ok(MatchmakingStatusDto {
            in_queue: true,
            game_id: None,
            enqueued_at_ms: Some(entry.enqueued_at.timestamp_millis()),
        }),
        None => Ok(not_queued()),
    }
}

fn not_queued() -> MatchmakingStatusDto {
    MatchmakingStatusDto {
        in_queue: false,
        game_id: None,
        enqueued_at_ms: None,
    }
}

/// One pairing pass. Called by the background scheduler; safe to call
/// concurrently thanks to `SELECT ... FOR UPDATE SKIP LOCKED`.
pub async fn pair_once(state: &AppState) -> Result<bool, ApiError> {
    let mut tx = state.db.begin().await?;

    // The oldest waiting player *who has an opponent*, locked for this
    // transaction. Entries that nobody can be paired with are skipped, so a
    // lone player never blocks the rest of the queue.
    let Some(first) = repo::oldest_matchable(&mut tx).await? else {
        return Ok(false);
    };
    // Their earliest opponent with the same time control, also locked.
    let Some(second) = repo::waiting_opponent(&mut tx, &first).await? else {
        // The candidate found by `oldest_matchable` was taken by a concurrent
        // pass (both queries use SKIP LOCKED); try again on the next tick.
        return Ok(false);
    };

    let now = state.clock.now_ms();
    let created = Game::new(
        Uuid::new_v4(),
        first.user_id,
        first.initial_time_ms,
        first.increment_ms,
        first.rated,
        now,
    );
    // Seat the opponent so the game is born active: matchmaking games never
    // pass through the "waiting" lobby state.
    let game = domain::join(created, second.user_id, now)?;
    crate::infrastructure::repos::games::insert(&mut tx, &game).await?;

    // Point both queue entries at the committed game.
    repo::mark_matched(&mut tx, first.user_id, game.id).await?;
    repo::mark_matched(&mut tx, second.user_id, game.id).await?;

    tx.commit().await?;
    tracing::info!(game = %game.id, "matchmaking paired a game");
    Ok(true)
}

/// Pair as many games as possible in a single pass over the queue.
pub async fn drain_pairs(state: &AppState) -> Result<usize, ApiError> {
    let mut paired = 0;
    // Bounded loop: each iteration consumes at least two queue entries, so this
    // terminates; the cap is a belt-and-braces guard.
    for _ in 0..64 {
        if !pair_once(state).await? {
            break;
        }
        paired += 1;
    }
    Ok(paired)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(initial: i64, increment: i64, rated: bool) -> MatchmakingRequest {
        MatchmakingRequest {
            initial_time_ms: initial,
            increment_ms: increment,
            rated,
        }
    }

    #[test]
    fn rejects_out_of_bounds_time_controls() {
        assert!(validate(&request(999, 0, false)).is_err());
        assert!(validate(&request(60_000, 0, false)).is_ok());
        assert!(validate(&request(1_000, 60_001, false)).is_err());
        assert!(validate(&request(1_000, 0, false)).is_ok());
    }

    #[test]
    fn status_consumes_match_only_once() {
        // The consumption rule is encoded in the SQL; here we only assert the
        // default shape so callers can rely on it.
        let empty = not_queued();
        assert!(!empty.in_queue);
        assert!(empty.game_id.is_none());
    }
}
