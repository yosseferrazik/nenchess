//! Tournament DTOs for the public API.
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Tournament lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TournamentStatusDto {
    Registration,
    Active,
    Finished,
}

/// Pairing system: everyone-plays-everyone, or score-grouped Swiss.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TournamentFormatDto {
    /// Single round robin (the circle method).
    #[default]
    RoundRobin,
    /// Fixed number of rounds (`ceil(log2(n))`), pairings by score.
    Swiss,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TournamentDto {
    pub id: Uuid,
    pub name: String,
    pub status: TournamentStatusDto,
    pub format: TournamentFormatDto,
    #[ts(type = "number")]
    pub time_control_initial_ms: i64,
    #[ts(type = "number")]
    pub time_control_increment_ms: i64,
    pub rated: bool,
    pub max_participants: i32,
    pub current_round: i32,
    pub total_rounds: i32,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    pub created_by: Uuid,
    #[ts(type = "number")]
    pub started_at_ms: Option<i64>,
    #[ts(type = "number")]
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateTournamentRequest {
    pub name: String,
    #[serde(default = "default_tc_initial")]
    #[ts(type = "number")]
    pub time_control_initial_ms: i64,
    #[serde(default)]
    #[ts(type = "number")]
    pub time_control_increment_ms: i64,
    #[serde(default = "default_rated")]
    pub rated: bool,
    #[serde(default = "default_max_participants")]
    pub max_participants: i32,
    #[serde(default)]
    pub format: TournamentFormatDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ParticipantDto {
    pub user_id: Uuid,
    pub username: String,
    pub rating_snapshot: i32,
    #[ts(type = "number")]
    pub points: f64,
    pub games_played: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PairingDto {
    pub id: Uuid,
    pub round_number: i32,
    pub white_id: Option<Uuid>,
    pub white_username: Option<String>,
    pub black_id: Option<Uuid>,
    pub black_username: Option<String>,
    pub game_id: Option<Uuid>,
    pub bye: bool,
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RoundDto {
    pub round_number: i32,
    pub pairings: Vec<PairingDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TournamentView {
    pub tournament: TournamentDto,
    pub participants: Vec<ParticipantDto>,
    pub rounds: Vec<RoundDto>,
    pub standings: Vec<ParticipantDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TournamentsResponse {
    pub tournaments: Vec<TournamentDto>,
}

fn default_tc_initial() -> i64 {
    600_000
}

fn default_rated() -> bool {
    true
}

fn default_max_participants() -> i32 {
    16
}
