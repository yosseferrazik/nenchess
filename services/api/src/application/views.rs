//! Assembly of `GameView` snapshots for HTTP responses and WebSocket events.

use std::collections::HashMap;

use game_core::{Color, PieceKind};
use protocol::{
    ClockDto, ColorDto, GameResultDto, GameStatusDto, GameView, MoveDto, PieceKindDto, PlayerDto,
    ResultReasonDto,
};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::{Game, GameStatus, MoveRecord, ResultReason};
use crate::error::ApiError;
use crate::infrastructure::repos::{games as games_repo, users as users_repo};

/// Build the authoritative snapshot of `game` as of `now_ms`.
pub async fn game_view(
    conn: &mut PgConnection,
    game: &Game,
    now_ms: i64,
) -> Result<GameView, ApiError> {
    let mut ids = vec![game.white_player_id];
    if let Some(black) = game.black_player_id {
        ids.push(black);
    }
    let usernames: HashMap<Uuid, String> = users_repo::usernames_for(conn, &ids).await?;
    let moves = games_repo::moves_for(conn, game.id).await?;
    Ok(build(game, now_ms, &usernames, moves))
}

fn build(
    game: &Game,
    now_ms: i64,
    usernames: &HashMap<Uuid, String>,
    moves: Vec<MoveRecord>,
) -> GameView {
    let player = |id: Uuid| {
        usernames.get(&id).map(|username| PlayerDto {
            user_id: id,
            username: username.clone(),
        })
    };
    let active = game.status == GameStatus::Active;

    // Clocks are present from the moment the game starts. While active they
    // are evaluated "as of now" so clients can interpolate locally from
    // `server_now_ms`; once finished they are frozen at their final values.
    let clock = (game.status != GameStatus::Waiting).then(|| ClockDto {
        white_ms: if active {
            game.clock.remaining_ms_at(Color::White, now_ms)
        } else {
            game.clock.white_ms
        },
        black_ms: if active {
            game.clock.remaining_ms_at(Color::Black, now_ms)
        } else {
            game.clock.black_ms
        },
        increment_ms: game.clock.increment_ms,
        turn: color_dto(game.clock.turn),
        server_now_ms: now_ms,
    });

    GameView {
        id: game.id,
        status: status_dto(game.status),
        version: game.version,
        fen: game.fen.clone(),
        turn: active.then(|| color_dto(game.turn)),
        white: player(game.white_player_id),
        black: game.black_player_id.and_then(player),
        clock,
        result: game.result.map(|r| GameResultDto {
            winner: r.winner.map(color_dto),
            reason: reason_dto(r.reason),
        }),
        move_count: game.move_count,
        initial_time_ms: game.initial_time_ms,
        increment_ms: game.increment_ms,
        rated: game.rated,
        moves: moves
            .into_iter()
            .map(|m| MoveDto {
                move_number: m.move_number,
                from: m.from.to_string(),
                to: m.to.to_string(),
                promotion: m.promotion.map(piece_dto),
                uci: m.uci,
            })
            .collect(),
        draw_offered_by: active
            .then_some(game.draw_offered_by)
            .flatten()
            .map(color_dto),
    }
}

pub fn color_dto(color: Color) -> ColorDto {
    match color {
        Color::White => ColorDto::White,
        Color::Black => ColorDto::Black,
    }
}

pub fn piece_dto(piece: PieceKind) -> PieceKindDto {
    match piece {
        PieceKind::Queen => PieceKindDto::Queen,
        PieceKind::Rook => PieceKindDto::Rook,
        PieceKind::Bishop => PieceKindDto::Bishop,
        PieceKind::Knight => PieceKindDto::Knight,
    }
}

fn status_dto(status: GameStatus) -> GameStatusDto {
    match status {
        GameStatus::Waiting => GameStatusDto::Waiting,
        GameStatus::Active => GameStatusDto::Active,
        GameStatus::Finished => GameStatusDto::Finished,
    }
}

fn reason_dto(reason: ResultReason) -> ResultReasonDto {
    match reason {
        ResultReason::Checkmate => ResultReasonDto::Checkmate,
        ResultReason::Stalemate => ResultReasonDto::Stalemate,
        ResultReason::Resignation => ResultReasonDto::Resignation,
        ResultReason::Flag => ResultReasonDto::Flag,
        ResultReason::FiftyMove => ResultReasonDto::FiftyMove,
        ResultReason::ThreefoldRepetition => ResultReasonDto::ThreefoldRepetition,
        ResultReason::InsufficientMaterial => ResultReasonDto::InsufficientMaterial,
        ResultReason::Abandonment => ResultReasonDto::Abandonment,
        ResultReason::Agreement => ResultReasonDto::Agreement,
    }
}
