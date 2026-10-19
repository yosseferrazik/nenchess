//! Read-side exports: PGN documents replayed from the immutable move records
//! and the caller's game history for profile pages.
//!
//! SAN comes from the rules kernel (`game_core::san`); the API never
//! re-implements chess notation. A game is exported by replaying exactly what
//! was played, from the starting position, record by record.

use game_core::{Color, MoveInput, STARTING_FEN};
use protocol::{
    ColorDto, GameHistoryEntryDto, GameHistoryResponse, GameOutcomeDto, GameResultDto, PlayerDto,
};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::application::views;
use crate::domain::{Game, GameStatus, MoveRecord, ResultReason};
use crate::error::ApiError;
use crate::infrastructure::repos::games as games_repo;

/// Maximum page size a client may request for history.
const HISTORY_MAX_LIMIT: i64 = 100;

/// Default page size when the client does not ask for one.
pub const HISTORY_DEFAULT_LIMIT: i64 = 20;

/// Build the PGN document of a game: header tags plus the movetext, each
/// recorded move rendered as SAN by the rules kernel.
///
/// Works for any game state: unfinished games export what has been played
/// with the result token `*`.
pub async fn game_pgn(conn: &mut PgConnection, game_id: Uuid) -> Result<String, ApiError> {
    let game = games_repo::find(conn, game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;
    let meta = games_repo::pgn_meta(conn, game_id)
        .await?
        .ok_or_else(|| ApiError::not_found("game not found"))?;
    let moves = games_repo::moves_for(conn, game_id).await?;

    let result_token = result_token(&game);
    let mut pgn = String::new();
    pgn.push_str("[Event \"NenChess\"]\n");
    pgn.push_str("[Site \"NenChess\"]\n");
    pgn.push_str(&format!(
        "[Date \"{}\"]\n",
        meta.created_at.format("%Y.%m.%d")
    ));
    pgn.push_str("[Round \"1\"]\n");
    pgn.push_str(&format!("[White \"{}\"]\n", meta.white_username));
    pgn.push_str(&format!(
        "[Black \"{}\"]\n",
        meta.black_username.as_deref().unwrap_or("?")
    ));
    pgn.push_str(&format!("[Result \"{result_token}\"]\n"));
    pgn.push_str(&format!(
        "[TimeControl \"{}+{}\"]\n",
        game.initial_time_ms.div_euclid(1000),
        game.increment_ms.div_euclid(1000)
    ));
    pgn.push('\n');
    pgn.push_str(&wrap_movetext(&movetext(&game, &moves, result_token)?));
    pgn.push('\n');
    Ok(pgn)
}

/// The caller's game history, newest first, clamped to a sane page size.
pub async fn game_history(
    conn: &mut PgConnection,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<GameHistoryResponse, ApiError> {
    let rows = games_repo::history_for(
        conn,
        user_id,
        limit.clamp(1, HISTORY_MAX_LIMIT),
        offset.max(0),
    )
    .await?;
    Ok(GameHistoryResponse {
        games: rows
            .into_iter()
            .map(|row| history_entry(row, user_id))
            .collect(),
    })
}

/// Replay the moves and produce SAN tokens with move numbers and the final
/// result token, ready for wrapping.
fn movetext(
    game: &Game,
    moves: &[MoveRecord],
    result_token: &str,
) -> Result<Vec<String>, ApiError> {
    let mut fen = STARTING_FEN.to_string();
    let mut tokens = Vec::with_capacity(moves.len() * 2 + 1);
    for record in moves {
        let mv = MoveInput {
            from: record.from,
            to: record.to,
            promotion: record.promotion,
        };
        // `move_number` counts plies; PGN numbers full moves and only before
        // white's move: ply 1 → "1.", ply 3 → "2.".
        if record.move_number % 2 == 1 {
            tokens.push(format!("{}.", (record.move_number + 1) / 2));
        }
        tokens.push(game_core::san(&fen, &mv).map_err(replay_divergence)?);
        fen = game_core::validate_and_apply(&fen, &mv)
            .map_err(replay_divergence)?
            .0;
    }
    debug_assert_eq!(
        game.move_count as usize,
        moves.len(),
        "records and game agree"
    );
    tokens.push(result_token.to_string());
    Ok(tokens)
}

/// A record that cannot be replayed means stored data disagrees with the
/// rules kernel: a server bug, not a client error.
fn replay_divergence(err: impl std::fmt::Display) -> ApiError {
    ApiError::internal(format!("pgn replay diverged: {err}"))
}

/// Join tokens into PGN movetext lines of at most 75 characters (SPGN §3.4).
fn wrap_movetext(tokens: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for token in tokens {
        if !line.is_empty() && line.len() + 1 + token.len() > 75 {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(token);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.join("\n")
}

/// The PGN result token: decisive, drawn, or `*` while unfinished.
fn result_token(game: &Game) -> &'static str {
    match (game.status, game.result.as_ref()) {
        (GameStatus::Finished, Some(result)) => match result.winner {
            Some(Color::White) => "1-0",
            Some(Color::Black) => "0-1",
            None => "1/2-1/2",
        },
        _ => "*",
    }
}

/// One history row as the caller sees it: their colour, the opponent, the
/// result and their own outcome derived from it.
fn history_entry(row: games_repo::HistoryRow, user_id: Uuid) -> GameHistoryEntryDto {
    let color = if row.white_player_id == user_id {
        ColorDto::White
    } else {
        ColorDto::Black
    };
    let opponent = match color {
        ColorDto::White => {
            row.black_player_id
                .zip(row.black_username)
                .map(|(opponent_id, username)| PlayerDto {
                    user_id: opponent_id,
                    username,
                })
        }
        ColorDto::Black => Some(PlayerDto {
            user_id: row.white_player_id,
            username: row.white_username,
        }),
    };

    let status = GameStatus::from_db(&row.status).expect("games.status is database-constrained");
    let result = (status == GameStatus::Finished).then(|| GameResultDto {
        winner: row
            .result_winner
            .as_deref()
            .and_then(|raw| raw.parse::<Color>().ok())
            .map(views::color_dto),
        reason: row
            .result_reason
            .as_deref()
            .and_then(ResultReason::from_db)
            .map(views::reason_dto)
            .expect("finished games always carry a constrained result reason"),
    });
    let outcome = result.map(|result| match result.winner {
        None => GameOutcomeDto::Draw,
        Some(winner) if winner == color => GameOutcomeDto::Win,
        Some(_) => GameOutcomeDto::Loss,
    });

    GameHistoryEntryDto {
        game_id: row.id,
        color,
        opponent,
        status: views::status_dto(status),
        result,
        outcome,
        rated: row.rated,
        initial_time_ms: row.initial_time_ms,
        increment_ms: row.increment_ms,
        move_count: row.move_count,
        started_at_ms: row.created_at.timestamp_millis(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use protocol::GameStatusDto;

    use super::*;

    fn history_row() -> games_repo::HistoryRow {
        games_repo::HistoryRow {
            id: Uuid::nil(),
            status: "finished".to_string(),
            result_winner: Some("white".to_string()),
            result_reason: Some("checkmate".to_string()),
            rated: true,
            initial_time_ms: 600_000,
            increment_ms: 0,
            move_count: 42,
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            white_player_id: Uuid::from_u128(1),
            black_player_id: Some(Uuid::from_u128(2)),
            white_username: "alice".to_string(),
            black_username: Some("bob".to_string()),
        }
    }

    #[test]
    fn history_scores_the_outcome_for_each_caller() {
        let white = history_entry(history_row(), Uuid::from_u128(1));
        assert_eq!(white.color, ColorDto::White);
        assert_eq!(white.outcome, Some(GameOutcomeDto::Win));
        assert_eq!(
            white.opponent.as_ref().map(|p| p.username.as_str()),
            Some("bob")
        );
        assert_eq!(white.status, GameStatusDto::Finished);

        let black = history_entry(history_row(), Uuid::from_u128(2));
        assert_eq!(black.color, ColorDto::Black);
        assert_eq!(black.outcome, Some(GameOutcomeDto::Loss));
        assert_eq!(
            black.opponent.as_ref().map(|p| p.username.as_str()),
            Some("alice")
        );
    }

    #[test]
    fn history_draws_and_unfinished_games_have_no_forced_outcome() {
        let mut row = history_row();
        row.result_winner = None;
        row.result_reason = Some("agreement".to_string());
        let entry = history_entry(row, Uuid::from_u128(1));
        assert_eq!(entry.result.as_ref().and_then(|r| r.winner), None);
        assert_eq!(entry.outcome, Some(GameOutcomeDto::Draw));

        let mut row = history_row();
        row.status = "active".to_string();
        row.result_winner = None;
        row.result_reason = None;
        let entry = history_entry(row, Uuid::from_u128(1));
        assert_eq!(entry.result, None);
        assert_eq!(entry.outcome, None);
    }

    #[test]
    fn movetext_lines_stay_within_75_characters() {
        // A long, plausible game: 60 full moves of captures and checks.
        let mut tokens = Vec::new();
        for number in (1..=60).rev() {
            tokens.push(format!("{number}."));
            tokens.push("Rxe8+".to_string());
            tokens.push(format!("{number}."));
            tokens.push("Nf3".to_string());
        }
        tokens.push("1/2-1/2".to_string());
        let wrapped = wrap_movetext(&tokens);
        for line in wrapped.lines() {
            assert!(line.len() <= 75, "line too long ({}): {line}", line.len());
        }
        // Nothing was lost: tokens come back in order.
        let rejoined: Vec<&str> = wrapped.split_whitespace().collect();
        let expected: Vec<&str> = tokens.iter().map(String::as_str).collect();
        assert_eq!(rejoined, expected);
    }
}
