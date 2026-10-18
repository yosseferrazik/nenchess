//! Game persistence. Row mapping is kept here so the domain never sees SQL.

use chrono::{DateTime, Utc};
use game_core::{Clock, Color, PieceKind, Square};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::{Game, GameResult, GameStatus, MoveRecord, ResultReason};

const GAME_COLUMNS: &str = "id, white_player_id, black_player_id, status, fen, side_to_move, \
     version, move_count, initial_time_ms, increment_ms, rated, white_time_ms, black_time_ms, \
     turn_started_at, result_winner, result_reason, position_hashes";

#[derive(Debug, sqlx::FromRow)]
struct GameRow {
    id: Uuid,
    white_player_id: Uuid,
    black_player_id: Option<Uuid>,
    status: String,
    fen: String,
    side_to_move: String,
    version: i64,
    move_count: i32,
    initial_time_ms: i64,
    increment_ms: i64,
    rated: bool,
    white_time_ms: i64,
    black_time_ms: i64,
    turn_started_at: DateTime<Utc>,
    result_winner: Option<String>,
    result_reason: Option<String>,
    position_hashes: Vec<i64>,
}

fn decode_err(msg: &str) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.to_string(),
    )))
}

impl GameRow {
    fn into_domain(self) -> Result<Game, sqlx::Error> {
        let status =
            GameStatus::from_db(&self.status).ok_or_else(|| decode_err("unknown game status"))?;
        let turn: Color = self
            .side_to_move
            .parse()
            .map_err(|_| decode_err("unknown side_to_move"))?;

        let result = match status {
            GameStatus::Finished => {
                let winner = match self.result_winner.as_deref() {
                    None => None,
                    Some(raw) => Some(
                        raw.parse()
                            .map_err(|_| decode_err("unknown result_winner"))?,
                    ),
                };
                let reason = self
                    .result_reason
                    .as_deref()
                    .and_then(ResultReason::from_db)
                    .ok_or_else(|| decode_err("missing/unknown result_reason"))?;
                Some(GameResult { winner, reason })
            }
            GameStatus::Waiting | GameStatus::Active => None,
        };

        Ok(Game {
            id: self.id,
            white_player_id: self.white_player_id,
            black_player_id: self.black_player_id,
            status,
            fen: self.fen,
            turn,
            version: self.version,
            move_count: self.move_count,
            initial_time_ms: self.initial_time_ms,
            increment_ms: self.increment_ms,
            rated: self.rated,
            clock: Clock {
                white_ms: self.white_time_ms,
                black_ms: self.black_time_ms,
                increment_ms: self.increment_ms,
                turn,
                turn_started_ms: self.turn_started_at.timestamp_millis(),
            },
            result,
            position_hashes: self.position_hashes,
        })
    }
}

pub async fn insert(conn: &mut PgConnection, game: &Game) -> Result<(), sqlx::Error> {
    let turn_started = DateTime::from_timestamp_millis(game.clock.turn_started_ms)
        .ok_or_else(|| decode_err("bad turn_started_ms"))?;
    let (winner, reason) = match &game.result {
        Some(r) => (r.winner.map(|c| c.as_str()), Some(r.reason.as_db())),
        None => (None, None),
    };
    sqlx::query(
        "INSERT INTO games (id, white_player_id, black_player_id, status, fen, side_to_move, \
         version, move_count, initial_time_ms, increment_ms, rated, white_time_ms, black_time_ms, \
         turn_started_at, result_winner, result_reason, position_hashes) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
    )
    .bind(game.id)
    .bind(game.white_player_id)
    .bind(game.black_player_id)
    .bind(game.status.as_db())
    .bind(&game.fen)
    .bind(game.turn.as_str())
    .bind(game.version)
    .bind(game.move_count)
    .bind(game.initial_time_ms)
    .bind(game.increment_ms)
    .bind(game.rated)
    .bind(game.clock.white_ms)
    .bind(game.clock.black_ms)
    .bind(turn_started)
    .bind(winner)
    .bind(reason)
    .bind(&game.position_hashes)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Game>, sqlx::Error> {
    sqlx::query_as::<_, GameRow>(&format!("SELECT {GAME_COLUMNS} FROM games WHERE id = $1"))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .map(GameRow::into_domain)
        .transpose()
}

/// `SELECT ... FOR UPDATE`: locks the row until the transaction ends. All
/// mutating flows go through this, so writes are serialized across processes.
pub async fn find_for_update(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<Game>, sqlx::Error> {
    sqlx::query_as::<_, GameRow>(&format!(
        "SELECT {GAME_COLUMNS} FROM games WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?
    .map(GameRow::into_domain)
    .transpose()
}

pub async fn update(conn: &mut PgConnection, game: &Game) -> Result<(), sqlx::Error> {
    let turn_started = DateTime::from_timestamp_millis(game.clock.turn_started_ms)
        .ok_or_else(|| decode_err("bad turn_started_ms"))?;
    let (winner, reason) = match &game.result {
        Some(r) => (r.winner.map(|c| c.as_str()), Some(r.reason.as_db())),
        None => (None, None),
    };
    let affected = sqlx::query(
        "UPDATE games SET black_player_id = $2, status = $3, fen = $4, side_to_move = $5, \
         version = $6, move_count = $7, white_time_ms = $8, black_time_ms = $9, \
         turn_started_at = $10, result_winner = $11, result_reason = $12, \
         position_hashes = $13, updated_at = now() \
         WHERE id = $1",
    )
    .bind(game.id)
    .bind(game.black_player_id)
    .bind(game.status.as_db())
    .bind(&game.fen)
    .bind(game.turn.as_str())
    .bind(game.version)
    .bind(game.move_count)
    .bind(game.clock.white_ms)
    .bind(game.clock.black_ms)
    .bind(turn_started)
    .bind(winner)
    .bind(reason)
    .bind(&game.position_hashes)
    .execute(conn)
    .await?
    .rows_affected();
    if affected != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// Open games in the lobby, newest first.
pub async fn list_open(conn: &mut PgConnection) -> Result<Vec<Game>, sqlx::Error> {
    let rows = sqlx::query_as::<_, GameRow>(&format!(
        "SELECT {GAME_COLUMNS} FROM games WHERE status = 'waiting' \
         ORDER BY created_at DESC LIMIT 50"
    ))
    .fetch_all(conn)
    .await?;
    rows.into_iter().map(GameRow::into_domain).collect()
}

pub async fn insert_move(
    conn: &mut PgConnection,
    game_id: Uuid,
    record: &MoveRecord,
    fen_after: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO game_moves (game_id, move_number, from_sq, to_sq, promotion, uci, fen_after) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(game_id)
    .bind(record.move_number)
    .bind(record.from.to_string())
    .bind(record.to.to_string())
    .bind(record.promotion.map(|p| p.uci_letter().to_string()))
    .bind(&record.uci)
    .bind(fen_after)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct MoveRow {
    move_number: i32,
    from_sq: String,
    to_sq: String,
    promotion: Option<String>,
    uci: String,
}

/// Full immutable move history of a game, oldest first.
pub async fn moves_for(
    conn: &mut PgConnection,
    game_id: Uuid,
) -> Result<Vec<MoveRecord>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MoveRow>(
        "SELECT move_number, from_sq, to_sq, promotion, uci FROM game_moves \
         WHERE game_id = $1 ORDER BY move_number ASC",
    )
    .bind(game_id)
    .fetch_all(conn)
    .await?;

    rows.into_iter()
        .map(|row| {
            let from: Square = row.from_sq.parse().map_err(|_| decode_err("bad from_sq"))?;
            let to: Square = row.to_sq.parse().map_err(|_| decode_err("bad to_sq"))?;
            let promotion = row
                .promotion
                .as_deref()
                .map(|s| {
                    s.chars()
                        .next()
                        .and_then(PieceKind::from_uci_letter)
                        .ok_or_else(|| decode_err("bad promotion"))
                })
                .transpose()?;
            Ok(MoveRecord {
                move_number: row.move_number,
                from,
                to,
                promotion,
                uci: row.uci,
            })
        })
        .collect()
}
