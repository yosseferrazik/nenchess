//! Games module: lobby operations (create, join, read, list).
//! Move/resign submission — the transactional, idempotent path — lives in
//! `application::moves`.

use protocol::{CreateGameRequest, GameView, GamesResponse};
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::views;
use crate::bootstrap::AppState;
use crate::domain::{self, Game};
use crate::error::ApiError;
use crate::infrastructure::repos::games as repo;

/// Create a game waiting for an opponent. The creator is white.
pub async fn create_game(
    state: &AppState,
    user_id: Uuid,
    req: CreateGameRequest,
) -> Result<GameView, ApiError> {
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

    let now = state.clock.now_ms();
    let game = Game::new(
        Uuid::new_v4(),
        user_id,
        req.initial_time_ms,
        req.increment_ms,
        req.rated,
        now,
    );

    let mut conn = state.db.acquire().await?;
    repo::insert(&mut conn, &game).await?;
    views::game_view(&mut conn, &game, now).await
}

/// Second player takes the black seat; the game starts and clocks are anchored.
pub async fn join_game(
    state: &AppState,
    user_id: Uuid,
    game_id: Uuid,
) -> Result<GameView, ApiError> {
    let _lock = state.registry.lock_for(game_id).await;
    let mut tx = state.db.begin().await?;

    let game = repo::find_for_update(&mut tx, game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;
    let now = state.clock.now_ms();
    let joined = domain::join(game, user_id, now)?;
    repo::update(&mut tx, &joined).await?;

    // Announce the arrival through the outbox so every subscriber (and any
    // future consumer) sees it after commit.
    let player = users_dto(&mut tx, user_id).await?;
    let msg = protocol::ServerMessage::PlayerJoined { game_id, player };
    crate::infrastructure::repos::outbox::enqueue(
        &mut tx,
        game_id,
        "game.player_joined",
        &serde_json::to_value(&msg)?,
    )
    .await?;

    let view = views::game_view(&mut tx, &joined, now).await?;
    tx.commit().await?;

    tracing::info!(game = %game_id, user = %user_id, "game started");
    Ok(view)
}

/// Read a game (any authenticated user; moves are restricted to participants).
pub async fn get_game(state: &AppState, game_id: Uuid) -> Result<GameView, ApiError> {
    let mut conn = state.db.acquire().await?;
    let game = repo::find(&mut conn, game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;
    views::game_view(&mut conn, &game, state.clock.now_ms()).await
}

/// Open games in the lobby.
pub async fn list_open_games(state: &AppState) -> Result<GamesResponse, ApiError> {
    let mut conn = state.db.acquire().await?;
    let open = repo::list_open(&mut conn).await?;
    let now = state.clock.now_ms();
    let mut games = Vec::with_capacity(open.len());
    for game in &open {
        games.push(views::game_view(&mut conn, game, now).await?);
    }
    Ok(GamesResponse { games })
}

async fn users_dto(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<protocol::PlayerDto, ApiError> {
    let row = crate::infrastructure::repos::users::find_by_id(conn, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user not found"))?;
    Ok(protocol::PlayerDto {
        user_id: row.id,
        username: row.username,
    })
}

/// Exposed for other slices that need a pool-scoped read.
pub async fn get_game_from_pool(pool: &PgPool, game_id: Uuid) -> Result<Game, ApiError> {
    let mut conn = pool.acquire().await?;
    repo::find(&mut conn, game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))
}
