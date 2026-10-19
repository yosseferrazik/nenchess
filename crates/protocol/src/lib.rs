//! Versioned wire contract shared by the HTTP and WebSocket interfaces.
//!
//! This crate is the single source of truth for frontend/backend
//! communication: the TypeScript types used by `apps/web` are generated from
//! these definitions by `cargo test -p protocol` (see `TS_RS_EXPORT_DIR` in
//! `.cargo/config.toml`).
//!
//! Rules of the contract:
//! * The client only sends *intentions* (commands); the server computes every
//!   consequence (results, ratings, clocks).
//! * Every real-time command carries a `command_id` for idempotency and, where
//!   it mutates state, an `expected_version` for optimistic concurrency.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

pub mod tournament;

pub use tournament::{
    CreateTournamentRequest, PairingDto, ParticipantDto, RoundDto, TournamentDto,
    TournamentStatusDto, TournamentView, TournamentsResponse,
};

/// Current protocol version. Bump on breaking changes and coordinate with the
/// frontend, which rejects connections with a mismatched version.
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Shared value objects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ColorDto {
    White,
    Black,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PieceKindDto {
    Queen,
    Rook,
    Bishop,
    Knight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    VersionConflict,
    NotYourTurn,
    IllegalMove,
    GameOver,
    GameFull,
    RateLimited,
    Internal,
}

/// Error body returned by every failing HTTP endpoint and by `error` frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ErrorDto {
    pub code: ErrorCode,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserDto,
    /// Session expiry as epoch milliseconds.
    #[ts(type = "number")]
    pub expires_at_ms: i64,
}

// ---------------------------------------------------------------------------
// Games
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GameStatusDto {
    Waiting,
    Active,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ResultReasonDto {
    Checkmate,
    Stalemate,
    Resignation,
    Flag,
    FiftyMove,
    ThreefoldRepetition,
    InsufficientMaterial,
    Abandonment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GameResultDto {
    /// `null` means a draw.
    pub winner: Option<ColorDto>,
    pub reason: ResultReasonDto,
}

/// Clock snapshot. `white_ms`/`black_ms` are the remaining times **at**
/// `server_now_ms`; clients interpolate from there and never send time back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClockDto {
    #[ts(type = "number")]
    pub white_ms: i64,
    #[ts(type = "number")]
    pub black_ms: i64,
    #[ts(type = "number")]
    pub increment_ms: i64,
    pub turn: ColorDto,
    #[ts(type = "number")]
    pub server_now_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlayerDto {
    pub user_id: Uuid,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MoveDto {
    pub move_number: i32,
    pub from: String,
    pub to: String,
    pub promotion: Option<PieceKindDto>,
    pub uci: String,
}

/// Move confirmed by the server, including the resulting position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppliedMoveDto {
    pub move_number: i32,
    pub from: String,
    pub to: String,
    pub promotion: Option<PieceKindDto>,
    pub uci: String,
    pub fen_after: String,
}

/// Full, authoritative snapshot of a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GameView {
    pub id: Uuid,
    pub status: GameStatusDto,
    /// Monotonically increasing state version used for optimistic concurrency.
    #[ts(type = "number")]
    pub version: i64,
    pub fen: String,
    /// Side to move; `null` while waiting or after the game finished.
    pub turn: Option<ColorDto>,
    pub white: Option<PlayerDto>,
    pub black: Option<PlayerDto>,
    /// `null` while the game is waiting for an opponent.
    pub clock: Option<ClockDto>,
    pub result: Option<GameResultDto>,
    pub move_count: i32,
    #[ts(type = "number")]
    pub initial_time_ms: i64,
    #[ts(type = "number")]
    pub increment_ms: i64,
    pub rated: bool,
    /// Full move history (immutable records), oldest first.
    pub moves: Vec<MoveDto>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateGameRequest {
    /// Default: 10 minutes.
    #[serde(default = "default_initial_time_ms")]
    #[ts(type = "number")]
    pub initial_time_ms: i64,
    #[serde(default)]
    #[ts(type = "number")]
    pub increment_ms: i64,
    #[serde(default)]
    pub rated: bool,
}

pub fn default_initial_time_ms() -> i64 {
    600_000
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GamesResponse {
    pub games: Vec<GameView>,
}

// ---------------------------------------------------------------------------
// Matchmaking
// ---------------------------------------------------------------------------

/// Request to enter (or re-enter) the matchmaking queue. Two players paired
/// share a time control; the colour is decided by the scheduler.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MatchmakingRequest {
    /// Default: 10 minutes.
    #[serde(default = "default_initial_time_ms")]
    #[ts(type = "number")]
    pub initial_time_ms: i64,
    #[serde(default)]
    #[ts(type = "number")]
    pub increment_ms: i64,
    #[serde(default)]
    pub rated: bool,
}

/// The caller's current matchmaking state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MatchmakingStatusDto {
    /// Whether the caller is waiting in the queue (unmatched).
    #[serde(default)]
    pub in_queue: bool,
    /// Set once an opponent was found: the game to open. The queue entry is
    /// consumed when this is returned.
    pub game_id: Option<Uuid>,
    /// When the caller entered the queue, for progress UIs.
    #[ts(type = "number")]
    pub enqueued_at_ms: Option<i64>,
}

/// Per-category rating snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RatingDto {
    pub category: String,
    pub rating: i32,
    pub games_played: i32,
}

/// Public profile with ratings across standard categories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProfileDto {
    pub user: UserDto,
    pub ratings: Vec<RatingDto>,
}

// ---------------------------------------------------------------------------
// WebSocket commands (client -> server)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ClientMessage {
    #[serde(rename = "ping")]
    Ping { command_id: Uuid },

    /// Subscribe this connection to a game's event stream. Returns the full
    /// `game.state` snapshot as `command_id` ack.
    #[serde(rename = "game.subscribe")]
    GameSubscribe { command_id: Uuid, game_id: Uuid },

    #[serde(rename = "game.unsubscribe")]
    GameUnsubscribe { command_id: Uuid, game_id: Uuid },

    /// Submit a move. Rejected when `expected_version` does not match the
    /// server state; `command_id` makes retries idempotent.
    #[serde(rename = "game.move")]
    GameMove {
        command_id: Uuid,
        game_id: Uuid,
        #[ts(type = "number")]
        expected_version: i64,
        #[serde(rename = "move")]
        mv: MoveInputDto,
    },

    /// Resign the game as the issuing player.
    #[serde(rename = "game.resign")]
    GameResign { command_id: Uuid, game_id: Uuid },
}

/// A move as the client expresses it. Coordinates only — the server validates
/// legality against the authoritative position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MoveInputDto {
    pub from: String,
    pub to: String,
    pub promotion: Option<PieceKindDto>,
}

// ---------------------------------------------------------------------------
// WebSocket events (server -> client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ServerMessage {
    #[serde(rename = "pong")]
    Pong { command_id: Uuid },

    #[serde(rename = "error")]
    Error {
        command_id: Option<Uuid>,
        code: ErrorCode,
        message: String,
    },

    /// Full snapshot (subscription ack and recovery after reconnect).
    #[serde(rename = "game.state")]
    GameState { command_id: Uuid, game: GameView },

    /// Confirmed move: ack for the mover, event for both players.
    #[serde(rename = "game.move_accepted")]
    MoveAccepted {
        command_id: Uuid,
        game: GameView,
        applied: AppliedMoveDto,
    },

    /// Terminal state reached by resignation, flag or other non-move ending.
    #[serde(rename = "game.ended")]
    GameEnded {
        command_id: Option<Uuid>,
        game: GameView,
    },

    #[serde(rename = "game.player_joined")]
    PlayerJoined { game_id: Uuid, player: PlayerDto },

    #[serde(rename = "game.player_left")]
    PlayerLeft { game_id: Uuid, user_id: Uuid },
}

impl ServerMessage {
    /// Game this event belongs to, if any (used by the outbox dispatcher).
    pub fn game_id(&self) -> Option<Uuid> {
        match self {
            ServerMessage::GameState { game, .. } => Some(game.id),
            ServerMessage::MoveAccepted { game, .. } => Some(game.id),
            ServerMessage::GameEnded { game, .. } => Some(game.id),
            ServerMessage::PlayerJoined { game_id, .. } => Some(*game_id),
            ServerMessage::PlayerLeft { game_id, .. } => Some(*game_id),
            ServerMessage::Pong { .. } | ServerMessage::Error { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_use_the_documented_wire_shape() {
        let cmd = ClientMessage::GameMove {
            command_id: Uuid::nil(),
            game_id: Uuid::nil(),
            expected_version: 18,
            mv: MoveInputDto {
                from: "e2".into(),
                to: "e4".into(),
                promotion: None,
            },
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["type"], "game.move");
        assert_eq!(json["expected_version"], 18);
        assert_eq!(json["move"]["from"], "e2");
        assert_eq!(json["move"]["promotion"], serde_json::Value::Null);
    }

    #[test]
    fn commands_roundtrip() {
        let cmd = ClientMessage::GameResign {
            command_id: Uuid::new_v4(),
            game_id: Uuid::new_v4(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
    }

    #[test]
    fn errors_carry_codes() {
        let err = ServerMessage::Error {
            command_id: None,
            code: ErrorCode::VersionConflict,
            message: "stale version".into(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "version_conflict");
    }
}
