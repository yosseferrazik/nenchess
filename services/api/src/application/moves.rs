//! The move pipeline — the most important flow in the platform:
//!
//! 1. per-game lock (serialize writers in-process)
//! 2. open transaction + `SELECT … FOR UPDATE` (serialize across processes)
//! 3. idempotency lookup (replay retried commands)
//! 4. domain transition (version → participation → turn → flag → legality)
//! 5. atomic persistence: state + move record + outbox event + idempotency key
//! 6. commit; the outbox dispatcher publishes afterwards (post-commit)

use game_core::MoveInput;
use protocol::ServerMessage;
use uuid::Uuid;

use crate::application::views;
use crate::bootstrap::AppState;
use crate::domain::{self, GameStatus, MoveOutcome};
use crate::error::ApiError;
use crate::infrastructure::repos::{games as games_repo, idempotency, outbox};
use crate::modules::{ratings, tournaments};

type DbTx<'a> = sqlx::Transaction<'a, sqlx::Postgres>;

pub struct MoveCommand {
    pub command_id: Uuid,
    pub game_id: Uuid,
    pub expected_version: i64,
    pub mv: MoveInput,
}

pub struct ResignCommand {
    pub command_id: Uuid,
    pub game_id: Uuid,
}

/// Submit a move. Returns the exact `ServerMessage` to send back to the
/// caller; for retries that message is the one originally stored.
pub async fn submit_move(
    state: &AppState,
    user_id: Uuid,
    cmd: MoveCommand,
) -> Result<ServerMessage, ApiError> {
    let _lock = state.registry.lock_for(cmd.game_id).await;
    let mut tx = state.db.begin().await?;

    // Row lock: serializes this writer with every other writer of the game.
    let game = games_repo::find_for_update(&mut tx, cmd.game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;

    // Retried command → replay the original confirmation.
    if let Some(stored) = stored_response(&mut tx, cmd.command_id).await? {
        tx.rollback().await.ok();
        return Ok(stored);
    }

    let now = state.clock.now_ms();
    let outcome = domain::apply_move(game, user_id, cmd.expected_version, &cmd.mv, now)?;

    // Atomic persistence: everything below commits together or not at all.
    let message = match outcome {
        MoveOutcome::Applied {
            game,
            applied,
            fen_after,
            ..
        } => {
            games_repo::update(&mut tx, &game).await?;
            games_repo::insert_move(&mut tx, game.id, &applied, &fen_after).await?;
            // A game that ended on this move settles its consequences — Elo
            // and any tournament pairing — in the same transaction as the
            // result.
            if game.status == GameStatus::Finished {
                ratings::settle(&mut tx, &game).await?;
                tournaments::settle_game(&mut tx, &game).await?;
            }
            let view = views::game_view(&mut tx, &game, now).await?;
            let applied_dto = protocol::AppliedMoveDto {
                move_number: applied.move_number,
                from: applied.from.to_string(),
                to: applied.to.to_string(),
                promotion: applied.promotion.map(views::piece_dto),
                uci: applied.uci,
                fen_after,
            };
            ServerMessage::MoveAccepted {
                command_id: cmd.command_id,
                game: view,
                applied: applied_dto,
            }
        }
        MoveOutcome::FlagExpired { game } => {
            // The flag fell before the move could be applied: the game ends
            // without a new move record.
            games_repo::update(&mut tx, &game).await?;
            ratings::settle(&mut tx, &game).await?;
            tournaments::settle_game(&mut tx, &game).await?;
            let view = views::game_view(&mut tx, &game, now).await?;
            ServerMessage::GameEnded {
                command_id: Some(cmd.command_id),
                game: view,
            }
        }
    };

    match commit_with_idempotency(
        &mut tx,
        cmd.command_id,
        user_id,
        "game.move",
        cmd.game_id,
        &message,
    )
    .await?
    {
        CommitOutcome::Committed => {
            tx.commit().await?;
            state
                .metrics
                .moves_committed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::debug!(
                game = %cmd.game_id,
                version = message_version(&message),
                "move committed"
            );
            Ok(message)
        }
        CommitOutcome::Replayed(stored) => {
            tx.rollback().await.ok();
            Ok(*stored)
        }
    }
}

/// Resign. Same pipeline as a move minus version/legality checks.
pub async fn submit_resign(
    state: &AppState,
    user_id: Uuid,
    cmd: ResignCommand,
) -> Result<ServerMessage, ApiError> {
    let _lock = state.registry.lock_for(cmd.game_id).await;
    let mut tx = state.db.begin().await?;

    let game = games_repo::find_for_update(&mut tx, cmd.game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;

    if let Some(stored) = stored_response(&mut tx, cmd.command_id).await? {
        tx.rollback().await.ok();
        return Ok(stored);
    }

    let now = state.clock.now_ms();
    let finished = domain::resign(game, user_id, now)?;
    games_repo::update(&mut tx, &finished).await?;
    ratings::settle(&mut tx, &finished).await?;
    tournaments::settle_game(&mut tx, &finished).await?;
    let view = views::game_view(&mut tx, &finished, now).await?;
    let message = ServerMessage::GameEnded {
        command_id: Some(cmd.command_id),
        game: view,
    };

    match commit_with_idempotency(
        &mut tx,
        cmd.command_id,
        user_id,
        "game.resign",
        cmd.game_id,
        &message,
    )
    .await?
    {
        CommitOutcome::Committed => {
            tx.commit().await?;
            state
                .metrics
                .moves_committed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(message)
        }
        CommitOutcome::Replayed(stored) => {
            tx.rollback().await.ok();
            Ok(*stored)
        }
    }
}

async fn stored_response(
    tx: &mut DbTx<'_>,
    command_id: Uuid,
) -> Result<Option<ServerMessage>, ApiError> {
    match idempotency::find(&mut *tx, command_id).await? {
        Some(value) => Ok(Some(serde_json::from_value(value)?)),
        None => Ok(None),
    }
}

enum CommitOutcome {
    Committed,
    /// Another writer claimed the command first; replay their response.
    /// Boxed to keep the enum small (the message carries a full game view).
    Replayed(Box<ServerMessage>),
}

/// Enqueue the outbox event and claim the idempotency key inside the same
/// transaction.
async fn commit_with_idempotency(
    tx: &mut DbTx<'_>,
    command_id: Uuid,
    user_id: Uuid,
    kind: &str,
    game_id: Uuid,
    message: &ServerMessage,
) -> Result<CommitOutcome, ApiError> {
    let payload = serde_json::to_value(message)?;
    outbox::enqueue(&mut *tx, game_id, event_type_of(message), &payload).await?;
    let claimed =
        idempotency::insert_if_absent(&mut *tx, command_id, user_id, kind, &payload).await?;
    if claimed {
        return Ok(CommitOutcome::Committed);
    }
    // Only reachable if a concurrent process already committed this exact
    // command: abort our own work and replay the stored response.
    let stored = idempotency::find(&mut *tx, command_id)
        .await?
        .ok_or_else(|| ApiError::internal("idempotency conflict without stored response"))?;
    Ok(CommitOutcome::Replayed(Box::new(serde_json::from_value(
        stored,
    )?)))
}

fn event_type_of(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::MoveAccepted { .. } => "game.move_accepted",
        ServerMessage::GameEnded { .. } => "game.ended",
        _ => "game.other",
    }
}

fn message_version(message: &ServerMessage) -> i64 {
    match message {
        ServerMessage::MoveAccepted { game, .. } | ServerMessage::GameEnded { game, .. } => {
            game.version
        }
        _ => -1,
    }
}
