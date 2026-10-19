//! Tournaments (stage 05): creation, joining, pairing and settlement.
//!
//! Scope of this slice:
//! * **Round-robin** format. A single group plays everyone else once; the
//!   schedule is generated with the circle method (a bye is handed to an odd
//!   player each round).
//! * **Swiss** format. `ceil(log2(n))` rounds; round 1 seeds the top half
//!   against the bottom half, later rounds are paired lazily (simplified
//!   Dutch) from the standings the moment the previous round is fully
//!   decided, preferring opponents nobody has faced yet and balancing colours.
//! * Lifecycle: `registration` → `active` → `finished`.
//! * `start` freezes the roster and emits the pairings; each pairing becomes a
//!   real `games` row so the existing authoritative move pipeline (clocks,
//!   legality, outbox, Elo) applies unchanged.
//! * When a tournament game ends, `settle_game` records the result in the
//!   pairing, bumps standings, and advances the round once every game of the
//!   current round is decided.
//!
//! The server is the only authority: clients join, start (creator only) and
//! play; they never report a score.

use std::collections::{HashMap, HashSet};

use protocol::{
    PairingDto, ParticipantDto, RoundDto, TournamentDto, TournamentFormatDto, TournamentStatusDto,
    TournamentView, TournamentsResponse,
};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::{self, Game, GameStatus};
use crate::error::ApiError;
use crate::infrastructure::repos::games as games_repo;
use crate::infrastructure::repos::ratings as ratings_repo;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TournamentRow {
    id: Uuid,
    name: String,
    status: String,
    format: String,
    time_control_initial_ms: i64,
    time_control_increment_ms: i64,
    rated: bool,
    max_participants: i32,
    current_round: i32,
    total_rounds: i32,
    created_by: Uuid,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct ParticipantRow {
    user_id: Uuid,
    username: String,
    rating_snapshot: i32,
    points: f64,
    games_played: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct PairingRow {
    id: Uuid,
    tournament_id: Uuid,
    round_number: i32,
    white_id: Option<Uuid>,
    black_id: Option<Uuid>,
    game_id: Option<Uuid>,
    bye: bool,
    result: Option<String>,
}

pub(crate) fn row_to_dto(r: TournamentRow) -> TournamentDto {
    TournamentDto {
        id: r.id,
        name: r.name,
        status: match r.status.as_str() {
            "active" => TournamentStatusDto::Active,
            "finished" => TournamentStatusDto::Finished,
            _ => TournamentStatusDto::Registration,
        },
        format: match r.format.as_str() {
            "swiss" => TournamentFormatDto::Swiss,
            _ => TournamentFormatDto::RoundRobin,
        },
        time_control_initial_ms: r.time_control_initial_ms,
        time_control_increment_ms: r.time_control_increment_ms,
        rated: r.rated,
        max_participants: r.max_participants,
        current_round: r.current_round,
        total_rounds: r.total_rounds,
        created_at_ms: r.created_at.timestamp_millis(),
        created_by: r.created_by,
        started_at_ms: r.started_at.map(|t| t.timestamp_millis()),
        finished_at_ms: r.finished_at.map(|t| t.timestamp_millis()),
    }
}

const TOURNAMENT_COLUMNS: &str = "id, name, status, format, time_control_initial_ms, time_control_increment_ms, \
     rated, max_participants, current_round, total_rounds, created_by, started_at, finished_at, created_at";

fn format_str(format: TournamentFormatDto) -> &'static str {
    match format {
        TournamentFormatDto::RoundRobin => "round_robin",
        TournamentFormatDto::Swiss => "swiss",
    }
}

// ---------------------------------------------------------------------------
// Pairing (pure)
// ---------------------------------------------------------------------------

/// One scheduled game: `None` on a side means a bye.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pairing {
    pub white: Option<usize>,
    pub black: Option<usize>,
}

/// Single round-robin schedule via the circle method.
///
/// Returns `n - 1` rounds (with a virtual bye slot when `n` is odd) where every
/// distinct pair of players meets exactly once. Colours alternate so a player
/// does not always defend the same side.
pub fn round_robin_schedule(n: usize) -> Vec<Vec<Pairing>> {
    if n < 2 {
        return Vec::new();
    }
    // Pad an odd field with a bye slot so the circle stays balanced.
    let bye = n;
    let m = if n.is_multiple_of(2) { n } else { n + 1 };
    let rounds = m - 1;
    let mut order: Vec<usize> = (0..m).collect();

    let mut schedule = Vec::with_capacity(rounds);
    for r in 0..rounds {
        let mut round = Vec::with_capacity(m / 2);
        for i in 0..m / 2 {
            let a = order[i];
            let b = order[m - 1 - i];
            // Alternate the fixed player's colour across rounds.
            let (white, black) = if r % 2 == 0 { (a, b) } else { (b, a) };
            round.push(Pairing {
                white: (white != bye).then_some(white),
                black: (black != bye).then_some(black),
            });
        }
        schedule.push(round);
        // Rotate every element except the anchor at position 0.
        let last = order.remove(m - 1);
        order.insert(1, last);
    }
    schedule
}

/// A player's Swiss résumé entering a round.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwissPlayer {
    pub user_id: Uuid,
    pub points: f64,
    pub rating: i32,
    /// Games played with the white pieces (byes excluded).
    pub whites: i32,
}

/// One Swiss pairing: a game, or — when `black` is `None` — a bye that scores
/// for `white`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwissPairing {
    pub white: Uuid,
    pub black: Option<Uuid>,
}

/// Swiss round 1 by seeding: the top half plays the bottom half in order
/// (colours alternating), and an odd field rests the lowest seed.
pub fn swiss_opening_round(n: usize) -> Vec<Pairing> {
    if n < 2 {
        return Vec::new();
    }
    let odd = n % 2 == 1;
    let active = n - usize::from(odd);
    let half = active / 2;
    let mut round: Vec<Pairing> = (0..half)
        .map(|i| {
            let (top, bottom) = (i, half + i);
            let (white, black) = if i % 2 == 0 {
                (top, bottom)
            } else {
                (bottom, top)
            };
            Pairing {
                white: Some(white),
                black: Some(black),
            }
        })
        .collect();
    if odd {
        // The lowest seed rests and scores a point.
        round.push(Pairing {
            white: Some(n - 1),
            black: None,
        });
    }
    round
}

/// Swiss plays enough rounds for a field of `n` to separate into a single
/// leader: `ceil(log2(n))` (2 → 1, 3 → 2, 5 → 3, 8 → 3).
pub fn swiss_total_rounds(n: usize) -> i32 {
    if n < 2 {
        return 0;
    }
    (u32::BITS - (n as u32 - 1).leading_zeros()) as i32
}

/// A later Swiss round (simplified Dutch): players ordered by score, then
/// rating, pair adjacently, preferring opponents they have never faced;
/// colours go to whoever has played fewer whites (ties: the higher-ranked
/// player). The leftover in an odd field takes the bye. When every remaining
/// candidate has already faced the anchor — possible only in tiny fields — a
/// rematch is the only way to keep the round whole.
pub fn swiss_round(players: &[SwissPlayer], history: &HashSet<(Uuid, Uuid)>) -> Vec<SwissPairing> {
    let mut remaining: Vec<SwissPlayer> = players.to_vec();
    remaining.sort_by(|a, b| {
        b.points
            .partial_cmp(&a.points)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.rating.cmp(&a.rating))
            .then(a.user_id.cmp(&b.user_id))
    });

    let mut round = Vec::with_capacity(remaining.len().div_ceil(2));
    while !remaining.is_empty() {
        let anchor = remaining.remove(0);
        if remaining.is_empty() {
            round.push(SwissPairing {
                white: anchor.user_id,
                black: None,
            });
            break;
        }
        let met = |p: &SwissPlayer| (anchor.user_id.min(p.user_id), anchor.user_id.max(p.user_id));
        let opponent = remaining
            .iter()
            .position(|p| !history.contains(&met(p)))
            .unwrap_or(0);
        let challenger = remaining.remove(opponent);
        let (white, black) = if anchor.whites <= challenger.whites {
            (anchor.user_id, challenger.user_id)
        } else {
            (challenger.user_id, anchor.user_id)
        };
        round.push(SwissPairing {
            white,
            black: Some(black),
        });
    }
    round
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn create_tournament(
    conn: &mut PgConnection,
    name: &str,
    time_control_initial_ms: i64,
    time_control_increment_ms: i64,
    rated: bool,
    max_participants: i32,
    format: TournamentFormatDto,
    created_by: Uuid,
) -> Result<TournamentDto, ApiError> {
    if name.trim().is_empty() || name.len() > 64 {
        return Err(ApiError::bad_request(
            "name must be between 1 and 64 characters",
        ));
    }
    if !(1_000..=3_600_000).contains(&time_control_initial_ms) {
        return Err(ApiError::bad_request(
            "initial_time_ms must be between 1000 and 3600000",
        ));
    }
    if !(0..=60_000).contains(&time_control_increment_ms) {
        return Err(ApiError::bad_request(
            "increment_ms must be between 0 and 60000",
        ));
    }
    if !(2..=64).contains(&max_participants) {
        return Err(ApiError::bad_request(
            "max_participants must be between 2 and 64",
        ));
    }

    let id = Uuid::new_v4();
    let row = sqlx::query_as::<_, TournamentRow>(&format!(
        "INSERT INTO tournaments (id, name, status, format, time_control_initial_ms, time_control_increment_ms, rated, max_participants, current_round, total_rounds, created_by) \
         VALUES ($1, $2, 'registration', $3, $4, $5, $6, $7, 0, 0, $8) \
         RETURNING {TOURNAMENT_COLUMNS}",
    ))
    .bind(id)
    .bind(name)
    .bind(format_str(format))
    .bind(time_control_initial_ms)
    .bind(time_control_increment_ms)
    .bind(rated)
    .bind(max_participants)
    .bind(created_by)
    .fetch_one(&mut *conn)
    .await?;

    // The creator is auto-enrolled as the first participant.
    let rating = rating_snapshot(
        conn,
        created_by,
        time_control_initial_ms,
        time_control_increment_ms,
    )
    .await?;
    sqlx::query(
        "INSERT INTO tournament_participants (tournament_id, user_id, rating_snapshot) VALUES ($1, $2, $3) \
         ON CONFLICT (tournament_id, user_id) DO NOTHING",
    )
    .bind(id)
    .bind(created_by)
    .bind(rating)
    .execute(&mut *conn)
    .await?;

    Ok(row_to_dto(row))
}

pub async fn list_tournaments(conn: &mut PgConnection) -> Result<TournamentsResponse, ApiError> {
    let rows = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments ORDER BY created_at DESC",
    ))
    .fetch_all(conn)
    .await?;
    Ok(TournamentsResponse {
        tournaments: rows.into_iter().map(row_to_dto).collect(),
    })
}

pub async fn join_tournament(
    conn: &mut PgConnection,
    tournament_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let t = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments WHERE id = $1 FOR UPDATE",
    ))
    .bind(tournament_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("tournament not found"))?;

    if t.status != "registration" {
        return Err(ApiError::bad_request(
            "tournament is not accepting registrations",
        ));
    }

    let joined = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tournament_participants WHERE tournament_id = $1 AND user_id = $2)",
    )
    .bind(tournament_id)
    .bind(user_id)
    .fetch_one(&mut *conn)
    .await?;
    if joined {
        return Err(ApiError::conflict("already registered for this tournament"));
    }

    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tournament_participants WHERE tournament_id = $1",
    )
    .bind(tournament_id)
    .fetch_one(&mut *conn)
    .await?;
    if count >= i64::from(t.max_participants) {
        return Err(ApiError::conflict("tournament is full"));
    }

    let rating = rating_snapshot(
        conn,
        user_id,
        t.time_control_initial_ms,
        t.time_control_increment_ms,
    )
    .await?;
    sqlx::query(
        "INSERT INTO tournament_participants (tournament_id, user_id, rating_snapshot) VALUES ($1, $2, $3)",
    )
    .bind(tournament_id)
    .bind(user_id)
    .bind(rating)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Freeze the roster, generate the schedule and open round 1.
///
/// Creator-only. Round-robin generates the full circle schedule upfront;
/// Swiss seeds round 1 (top half vs bottom half) and pairs the remaining
/// `ceil(log2(n)) - 1` rounds lazily as results land. Every non-bye pairing
/// becomes an authoritative `games` row so the normal move pipeline runs; byes
/// are awarded immediately.
pub async fn start_tournament(
    conn: &mut PgConnection,
    tournament_id: Uuid,
    user_id: Uuid,
    now_ms: i64,
) -> Result<TournamentDto, ApiError> {
    let t = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments WHERE id = $1 FOR UPDATE",
    ))
    .bind(tournament_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("tournament not found"))?;

    if t.created_by != user_id {
        return Err(ApiError::forbidden(
            "only the organizer can start the tournament",
        ));
    }
    if t.status != "registration" {
        return Err(ApiError::bad_request("tournament already started"));
    }

    // Roster ordered by rating (desc) so the circle method and the Swiss
    // seeding spread strong players; ties broken by user_id for a
    // deterministic schedule.
    let roster = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM tournament_participants WHERE tournament_id = $1 \
         ORDER BY rating_snapshot DESC, user_id",
    )
    .bind(tournament_id)
    .fetch_all(&mut *conn)
    .await?;
    if roster.len() < 2 {
        return Err(ApiError::bad_request(
            "need at least two participants to start",
        ));
    }

    // Round-robin pre-generates every round; Swiss opens only the seeded
    // first round and pairs the rest on demand.
    let schedule: Vec<Vec<Pairing>> = match t.format.as_str() {
        "swiss" => vec![swiss_opening_round(roster.len())],
        _ => round_robin_schedule(roster.len()),
    };
    let total_rounds = if t.format == "swiss" {
        swiss_total_rounds(roster.len())
    } else {
        schedule.len() as i32
    };

    for (idx, round) in schedule.iter().enumerate() {
        let round_number = (idx + 1) as i32;
        let round_id = Uuid::new_v4();
        let is_current = round_number == 1;
        sqlx::query(
            "INSERT INTO tournament_rounds (id, tournament_id, round_number, started_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(round_id)
        .bind(tournament_id)
        .bind(round_number)
        .bind(is_current.then_some(chrono::DateTime::from_timestamp_millis(now_ms)))
        .execute(&mut *conn)
        .await?;

        for pairing in round {
            let white_id = pairing.white.map(|i| roster[i]);
            let black_id = pairing.black.map(|i| roster[i]);
            let bye = white_id.is_none() || black_id.is_none();

            let pairing_id = Uuid::new_v4();
            let game_id = if bye {
                None
            } else {
                // Build an active two-seat game through the domain so clocks
                // and the starting position are correct.
                let created = Game::new(
                    Uuid::new_v4(),
                    white_id.expect("bye handled above"),
                    t.time_control_initial_ms,
                    t.time_control_increment_ms,
                    t.rated,
                    now_ms,
                );
                let game = domain::join(created, black_id.expect("bye handled above"), now_ms)?;
                games_repo::insert(&mut *conn, &game).await?;
                Some(game.id)
            };

            sqlx::query(
                "INSERT INTO tournament_pairings (id, tournament_id, round_id, round_number, white_id, black_id, game_id, bye, result) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(pairing_id)
            .bind(tournament_id)
            .bind(round_id)
            .bind(round_number)
            .bind(white_id)
            .bind(black_id)
            .bind(game_id)
            .bind(bye)
            .bind(bye.then_some("bye"))
            .execute(&mut *conn)
            .await?;

            // A bye scores immediately: the absent opponent forfeits.
            if bye {
                let winner = white_id.or(black_id).expect("bye has a winner");
                award_points(&mut *conn, tournament_id, winner, 1.0).await?;
            }
        }
    }

    sqlx::query(
        "UPDATE tournaments SET status = 'active', current_round = 1, started_at = $2, total_rounds = $3, updated_at = now() WHERE id = $1",
    )
    .bind(tournament_id)
    .bind(chrono::DateTime::from_timestamp_millis(now_ms))
    .bind(total_rounds)
    .execute(&mut *conn)
    .await?;

    tracing::info!(tournament = %tournament_id, rounds = total_rounds, "tournament started");
    let updated = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments WHERE id = $1",
    ))
    .bind(tournament_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok(row_to_dto(updated))
}

/// Record a finished tournament game: result, standings, and round advance.
///
/// Called inside the same transaction that finishes the game. A no-op for
/// games that are not part of a tournament.
pub async fn settle_game(conn: &mut PgConnection, game: &Game) -> Result<(), ApiError> {
    if game.status != GameStatus::Finished {
        return Ok(());
    }
    let Some(pairing) = sqlx::query_as::<_, PairingRow>(
        "SELECT id, tournament_id, round_number, white_id, black_id, game_id, bye, result \
         FROM tournament_pairings WHERE game_id = $1 FOR UPDATE",
    )
    .bind(game.id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(()); // not a tournament game
    };
    if pairing.result.is_some() {
        return Ok(()); // already settled (idempotent)
    }

    let white_id = pairing.white_id.expect("tournament game has two seats");
    let black_id = pairing.black_id.expect("tournament game has two seats");
    let result = match game.result.expect("finished game has a result").winner {
        None => "draw",
        Some(game_core::Color::White) => "white_win",
        Some(game_core::Color::Black) => "black_win",
    };
    sqlx::query("UPDATE tournament_pairings SET result = $2, updated_at = now() WHERE id = $1")
        .bind(pairing.id)
        .bind(result)
        .execute(&mut *conn)
        .await?;

    let (white_pts, black_pts) = match result {
        "white_win" => (1.0, 0.0),
        "black_win" => (0.0, 1.0),
        _ => (0.5, 0.5),
    };
    let tid = pairing.tournament_id;
    award_points(&mut *conn, tid, white_id, white_pts).await?;
    award_points(&mut *conn, tid, black_id, black_pts).await?;

    advance_if_round_complete(conn, game).await
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

pub async fn get_tournament(
    conn: &mut PgConnection,
    tournament_id: Uuid,
) -> Result<TournamentView, ApiError> {
    let row = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments WHERE id = $1",
    ))
    .bind(tournament_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::not_found("tournament not found"))?;

    let participants = sqlx::query_as::<_, ParticipantRow>(
        "SELECT p.user_id, u.username, p.rating_snapshot, p.points, p.games_played \
         FROM tournament_participants p JOIN users u ON u.id = p.user_id \
         WHERE p.tournament_id = $1 ORDER BY u.username",
    )
    .bind(tournament_id)
    .fetch_all(&mut *conn)
    .await?;

    let pairings = sqlx::query_as::<_, PairingRow>(
        "SELECT id, tournament_id, round_number, white_id, black_id, game_id, bye, result \
         FROM tournament_pairings WHERE tournament_id = $1 ORDER BY round_number, created_at",
    )
    .bind(tournament_id)
    .fetch_all(&mut *conn)
    .await?;

    let usernames = username_map(conn, &participants).await?;
    let participants_dto: Vec<ParticipantDto> = participants
        .iter()
        .map(|p| participant_dto(p, &usernames))
        .collect();

    let mut rounds: HashMap<i32, Vec<PairingDto>> = HashMap::new();
    for p in &pairings {
        rounds.entry(p.round_number).or_default().push(PairingDto {
            id: p.id,
            round_number: p.round_number,
            white_id: p.white_id,
            white_username: p.white_id.and_then(|id| usernames.get(&id).cloned()),
            black_id: p.black_id,
            black_username: p.black_id.and_then(|id| usernames.get(&id).cloned()),
            game_id: p.game_id,
            bye: p.bye,
            result: p.result.clone(),
        });
    }
    let mut rounds: Vec<RoundDto> = rounds
        .into_iter()
        .map(|(round_number, pairings)| RoundDto {
            round_number,
            pairings,
        })
        .collect();
    rounds.sort_by_key(|r| r.round_number);

    Ok(TournamentView {
        tournament: row_to_dto(row),
        participants: participants_dto.clone(),
        rounds,
        standings: standings(&participants, &pairings, &usernames),
    })
}

/// Standings: points desc, then Buchholz (sum of opponents' points), then
/// rating, then username for full determinism.
fn standings(
    participants: &[ParticipantRow],
    pairings: &[PairingRow],
    usernames: &HashMap<Uuid, String>,
) -> Vec<ParticipantDto> {
    let points: HashMap<Uuid, f64> = participants.iter().map(|p| (p.user_id, p.points)).collect();

    // Buchholz: each player's opponents' final points, summed.
    let mut buchholz: HashMap<Uuid, f64> = HashMap::new();
    for p in pairings {
        if p.bye {
            continue;
        }
        if let (Some(w), Some(b)) = (p.white_id, p.black_id) {
            *buchholz.entry(w).or_default() += points.get(&b).copied().unwrap_or(0.0);
            *buchholz.entry(b).or_default() += points.get(&w).copied().unwrap_or(0.0);
        }
    }

    // Sort the rows, then map to DTOs; the tiebreak reads each row's own
    // fields, so `ParticipantRow::username` stays in use here.
    let mut rows: Vec<&ParticipantRow> = participants.iter().collect();
    rows.sort_by(|a, b| {
        b.points
            .partial_cmp(&a.points)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                buchholz
                    .get(&b.user_id)
                    .unwrap_or(&0.0)
                    .partial_cmp(buchholz.get(&a.user_id).unwrap_or(&0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(b.rating_snapshot.cmp(&a.rating_snapshot))
            .then(a.username.cmp(&b.username))
    });
    rows.iter().map(|p| participant_dto(p, usernames)).collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn award_points(
    conn: &mut PgConnection,
    tournament_id: Uuid,
    user_id: Uuid,
    delta: f64,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE tournament_participants SET points = points + $3, games_played = games_played + 1 \
         WHERE tournament_id = $1 AND user_id = $2",
    )
    .bind(tournament_id)
    .bind(user_id)
    .bind(delta)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn advance_if_round_complete(conn: &mut PgConnection, game: &Game) -> Result<(), ApiError> {
    let Some(pairing) = sqlx::query_as::<_, PairingRow>(
        "SELECT id, tournament_id, round_number, white_id, black_id, game_id, bye, result \
         FROM tournament_pairings WHERE game_id = $1",
    )
    .bind(game.id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(());
    };
    let tournament_id = pairing.tournament_id;
    let t = sqlx::query_as::<_, TournamentRow>(&format!(
        "SELECT {TOURNAMENT_COLUMNS} FROM tournaments WHERE id = $1 FOR UPDATE",
    ))
    .bind(tournament_id)
    .fetch_one(&mut *conn)
    .await?;

    let undecided = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tournament_pairings \
         WHERE tournament_id = $1 AND round_number = $2 AND result IS NULL",
    )
    .bind(tournament_id)
    .bind(pairing.round_number)
    .fetch_one(&mut *conn)
    .await?;
    if undecided > 0 {
        return Ok(());
    }

    let next_round = pairing.round_number + 1;
    if next_round > t.total_rounds {
        sqlx::query(
            "UPDATE tournaments SET status = 'finished', finished_at = now(), updated_at = now() WHERE id = $1",
        )
        .bind(tournament_id)
        .execute(&mut *conn)
        .await?;
        tracing::info!(tournament = %tournament_id, "tournament finished");
        return Ok(());
    }

    // Swiss pairs on demand: the next round's matchups only exist once the
    // previous one is fully decided. The tournament row is locked above, so
    // the generation runs exactly once per round.
    if t.format == "swiss" {
        generate_swiss_round(conn, tournament_id, next_round).await?;
    }

    // Open the next round: attach a game to each undecided (non-bye) pairing.
    let pending = sqlx::query_as::<_, PairingRow>(
        "SELECT id, tournament_id, round_number, white_id, black_id, game_id, bye, result \
         FROM tournament_pairings WHERE tournament_id = $1 AND round_number = $2 ORDER BY created_at",
    )
    .bind(tournament_id)
    .bind(next_round)
    .fetch_all(&mut *conn)
    .await?;
    for p in pending {
        if p.bye {
            if let Some(winner) = p.white_id.or(p.black_id) {
                sqlx::query("UPDATE tournament_pairings SET result = 'bye' WHERE id = $1")
                    .bind(p.id)
                    .execute(&mut *conn)
                    .await?;
                award_points(conn, tournament_id, winner, 1.0).await?;
            }
            continue;
        }
        let white_id = p.white_id.expect("non-bye has white");
        let black_id = p.black_id.expect("non-bye has black");
        let created = Game::new(
            Uuid::new_v4(),
            white_id,
            t.time_control_initial_ms,
            t.time_control_increment_ms,
            t.rated,
            game.clock.turn_started_ms,
        );
        let new_game = domain::join(created, black_id, game.clock.turn_started_ms)?;
        games_repo::insert(conn, &new_game).await?;
        sqlx::query("UPDATE tournament_pairings SET game_id = $2 WHERE id = $1")
            .bind(p.id)
            .bind(new_game.id)
            .execute(&mut *conn)
            .await?;
    }

    sqlx::query("UPDATE tournaments SET current_round = $2, updated_at = now() WHERE id = $1")
        .bind(tournament_id)
        .bind(next_round)
        .execute(&mut *conn)
        .await?;
    tracing::info!(tournament = %tournament_id, round = next_round, "tournament round advanced");
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct SwissRow {
    user_id: Uuid,
    rating_snapshot: i32,
    points: f64,
}

/// Pair and insert the next Swiss round from the standings so far. The rows
/// land with `game_id IS NULL` and undecided results; the shared open-the-round
/// loop right after attaches the games and scores the byes, exactly as if the
/// round had been pre-generated like a round robin.
async fn generate_swiss_round(
    conn: &mut PgConnection,
    tournament_id: Uuid,
    round_number: i32,
) -> Result<(), ApiError> {
    let players = sqlx::query_as::<_, SwissRow>(
        "SELECT user_id, rating_snapshot, points FROM tournament_participants \
         WHERE tournament_id = $1",
    )
    .bind(tournament_id)
    .fetch_all(&mut *conn)
    .await?;

    // Every earlier pairing is decided by construction (we only advance once
    // the current round has no undecided row). Byes carry no colours; every
    // other result feeds the score, colour and rematch bookkeeping.
    let prior = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT white_id, black_id FROM tournament_pairings \
         WHERE tournament_id = $1 AND bye = false \
           AND white_id IS NOT NULL AND black_id IS NOT NULL",
    )
    .bind(tournament_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut history = HashSet::new();
    let mut whites: HashMap<Uuid, i32> = HashMap::new();
    for (white, black) in prior {
        *whites.entry(white).or_default() += 1;
        history.insert((white.min(black), white.max(black)));
    }

    let roster: Vec<SwissPlayer> = players
        .into_iter()
        .map(|p| SwissPlayer {
            user_id: p.user_id,
            points: p.points,
            rating: p.rating_snapshot,
            whites: whites.get(&p.user_id).copied().unwrap_or(0),
        })
        .collect();

    let round_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tournament_rounds (id, tournament_id, round_number, started_at) \
         VALUES ($1, $2, $3, NULL)",
    )
    .bind(round_id)
    .bind(tournament_id)
    .bind(round_number)
    .execute(&mut *conn)
    .await?;

    for pairing in swiss_round(&roster, &history) {
        sqlx::query(
            "INSERT INTO tournament_pairings (id, tournament_id, round_id, round_number, white_id, black_id, game_id, bye, result) \
             VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, NULL)",
        )
        .bind(Uuid::new_v4())
        .bind(tournament_id)
        .bind(round_id)
        .bind(round_number)
        .bind(pairing.white)
        .bind(pairing.black)
        .bind(pairing.black.is_none())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn username_map(
    conn: &mut PgConnection,
    participants: &[ParticipantRow],
) -> Result<HashMap<Uuid, String>, ApiError> {
    let ids: Vec<Uuid> = participants.iter().map(|p| p.user_id).collect();
    let rows =
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, username FROM users WHERE id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows.into_iter().collect())
}

fn participant_dto(p: &ParticipantRow, usernames: &HashMap<Uuid, String>) -> ParticipantDto {
    ParticipantDto {
        user_id: p.user_id,
        username: usernames
            .get(&p.user_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        rating_snapshot: p.rating_snapshot,
        points: p.points,
        games_played: p.games_played,
    }
}

async fn rating_snapshot(
    conn: &mut PgConnection,
    user_id: Uuid,
    initial_time_ms: i64,
    increment_ms: i64,
) -> Result<i32, sqlx::Error> {
    let category = crate::modules::ratings::category_for(initial_time_ms, increment_ms);
    ratings_repo::ensure_exists(conn, user_id, &[category]).await?;
    let map = ratings_repo::get_ratings_map(conn, user_id).await?;
    Ok(map.get(category).map(|(r, _)| *r).unwrap_or(1500))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_is_empty_for_tiny_fields() {
        assert!(round_robin_schedule(0).is_empty());
        assert!(round_robin_schedule(1).is_empty());
    }

    #[test]
    fn even_field_meets_every_pair_once() {
        let n = 4;
        let schedule = round_robin_schedule(n);
        assert_eq!(schedule.len(), n - 1);
        let mut seen = std::collections::HashSet::new();
        for round in &schedule {
            assert_eq!(round.len(), n / 2);
            for p in round {
                let (w, b) = (
                    p.white.expect("even field has no byes"),
                    p.black.expect("even field has no byes"),
                );
                let key = (w.min(b), w.max(b));
                assert!(seen.insert(key), "pair {key:?} played twice");
            }
        }
        assert_eq!(seen.len(), n * (n - 1) / 2);
    }

    #[test]
    fn odd_field_hands_out_one_bye_per_round() {
        let n = 5;
        let schedule = round_robin_schedule(n);
        assert_eq!(schedule.len(), n); // n is odd → n rounds
        for round in &schedule {
            let byes = round
                .iter()
                .filter(|p| p.white.is_none() || p.black.is_none())
                .count();
            assert_eq!(byes, 1, "exactly one bye per round");
        }
    }

    #[test]
    fn colours_alternate_for_the_anchor_player() {
        let schedule = round_robin_schedule(4);
        // Player 0 anchors position 0; their colour should flip across rounds.
        let anchor_colours: Vec<bool> = schedule
            .iter()
            .map(|round| {
                round
                    .iter()
                    .find(|p| p.white == Some(0) || p.black == Some(0))
                    .map(|p| p.white == Some(0))
                    .unwrap()
            })
            .collect();
        assert!(
            anchor_colours.windows(2).any(|w| w[0] != w[1]),
            "anchor must not always be white"
        );
    }

    #[test]
    fn swiss_rounds_cover_a_doubling_field() {
        assert_eq!(swiss_total_rounds(0), 0);
        assert_eq!(swiss_total_rounds(1), 0);
        assert_eq!(swiss_total_rounds(2), 1);
        assert_eq!(swiss_total_rounds(3), 2);
        assert_eq!(swiss_total_rounds(4), 2);
        assert_eq!(swiss_total_rounds(5), 3);
        assert_eq!(swiss_total_rounds(8), 3);
        assert_eq!(swiss_total_rounds(9), 4);
    }

    #[test]
    fn swiss_opening_splits_the_halves_and_rests_the_lowest_seed() {
        let round = swiss_opening_round(4);
        assert_eq!(round.len(), 2);
        for p in &round {
            let (white, black) = (p.white.expect("seeded pair"), p.black.expect("seeded pair"));
            // A top-half seed (0,1) always meets a bottom-half seed (2,3).
            assert_eq!(white < 2, black >= 2, "halves must clash, never within");
        }

        let round = swiss_opening_round(3);
        let bye = round
            .iter()
            .find(|p| p.black.is_none())
            .expect("odd field rests someone");
        assert_eq!(bye.white, Some(2), "the lowest seed takes the bye");
        assert_eq!(
            round.iter().filter(|p| p.black.is_some()).count(),
            1,
            "the other two play"
        );

        assert!(swiss_opening_round(1).is_empty());
    }

    #[test]
    fn swiss_round_pairs_by_score_avoids_rematches_and_balances_colours() {
        let (a, b, c, d) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let players = vec![
            SwissPlayer {
                user_id: a,
                points: 3.0,
                rating: 1600,
                whites: 1,
            },
            SwissPlayer {
                user_id: b,
                points: 3.0,
                rating: 1550,
                whites: 0,
            },
            SwissPlayer {
                user_id: c,
                points: 1.0,
                rating: 1500,
                whites: 0,
            },
            SwissPlayer {
                user_id: d,
                points: 0.0,
                rating: 1500,
                whites: 1,
            },
        ];

        // Everyone meets everyone on a clean slate: adjacent score pairs.
        let round = swiss_round(&players, &HashSet::new());
        assert_eq!(round.len(), 2);
        let pairs: Vec<(Uuid, Uuid)> = round
            .iter()
            .map(|p| {
                let (w, b) = (p.white, p.black.expect("clean slate has no byes"));
                (w.min(b), w.max(b))
            })
            .collect();
        assert!(
            pairs.contains(&(a.min(b), a.max(b))),
            "leaders meet first: {pairs:?}"
        );
        assert!(
            pairs.contains(&(c.min(d), c.max(d))),
            "chasers meet first: {pairs:?}"
        );

        // The leaders already met: the anchors drop to the nearest opponent
        // they have not faced, and colours follow the white counts.
        let mut history = HashSet::new();
        history.insert((a.min(b), a.max(b)));
        let round = swiss_round(&players, &history);
        let ac = round
            .iter()
            .find(|p| p.black == Some(a) || p.black == Some(c))
            .expect("a pairs with c");
        assert!(
            (ac.white == a && ac.black == Some(c)) || (ac.white == c && ac.black == Some(a)),
            "the leaders fall through to c: {ac:?}"
        );
        assert_eq!(
            ac.white, c,
            "c has played fewer whites and takes the colour"
        );
        assert!(
            round.iter().any(|p| p.black == Some(d)),
            "b pairs with the remaining chaser"
        );

        // Odd leftover takes the bye.
        let round = swiss_round(&players[..3], &history);
        let bye = round.iter().find(|p| p.black.is_none()).expect("a bye");
        // Score order: a, b, c — a pairs with c (b is met), b rests.
        assert_eq!(bye.white, b);
    }
}
