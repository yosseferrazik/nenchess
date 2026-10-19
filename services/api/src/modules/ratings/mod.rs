//! Ratings (Elo) — per-category, computed by the server.
//!
//! Rules (fixed in the skeleton and honoured here):
//! * The server computes rating changes — clients never send ELO.
//! * `player_ratings` rows are created on demand (default 1500).
//! * Every change appends `rating_history` (before/after) in the **same
//!   transaction** as the game result, so a crash can never settle a game
//!   without rating it (or vice versa).
//!
//! Categories follow the usual online convention, keyed off the initial time
//! control: `bullet` (< 3 min), `blitz` (< 10 min), `rapid` (>= 10 min).

use std::collections::HashMap;

use protocol::{ProfileDto, RatingDto};
use sqlx::PgConnection;

use crate::domain::{Game, GameStatus};
use crate::error::ApiError;
use crate::infrastructure::repos::ratings as repo;

/// K-factor: provisional players move fast, established ones slower, and the
/// very top even slower (FIDE-style).
const K_PROVISIONAL: f64 = 32.0;
const K_ESTABLISHED: f64 = 16.0;
const K_MASTER: f64 = 8.0;
const PROVISIONAL_GAMES: i32 = 30;
const MASTER_RATING: i32 = 2400;

const CATEGORIES: [&str; 3] = ["bullet", "blitz", "rapid"];
pub const DEFAULT_RATING: i32 = 1500;

/// Expected game length is `initial + ESTIMATE_INCREMENT_FACTOR * increment`;
/// below `BULLET_ESTIMATE_MAX` it is bullet, below `BLITZ_ESTIMATE_MAX` blitz,
/// otherwise rapid (Lichess-style thresholds: 3+2 is blitz, 1+0 is bullet).
const ESTIMATE_INCREMENT_FACTOR: i64 = 40;
const BULLET_ESTIMATE_MAX: i64 = 180_000;
const BLITZ_ESTIMATE_MAX: i64 = 600_000;

/// Map a time control to a rating category.
pub fn category_for(initial_time_ms: i64, increment_ms: i64) -> &'static str {
    let estimate = initial_time_ms.saturating_add(ESTIMATE_INCREMENT_FACTOR * increment_ms);
    if estimate < BULLET_ESTIMATE_MAX {
        "bullet"
    } else if estimate < BLITZ_ESTIMATE_MAX {
        "blitz"
    } else {
        "rapid"
    }
}

/// The same mapping as [`category_for`], for callers stuck in SQL (the
/// matchmaking pairing query needs the anchor's category before the anchor is
/// known). Built from the same constants so the two definitions cannot drift
/// apart silently; the e2e suite exercises the SQL path end to end.
pub fn category_sql(initial_expr: &str, increment_expr: &str) -> String {
    let estimate = format!("{initial_expr} + {ESTIMATE_INCREMENT_FACTOR} * {increment_expr}");
    format!(
        "CASE WHEN {estimate} < {BULLET_ESTIMATE_MAX} THEN 'bullet' \
         WHEN {estimate} < {BLITZ_ESTIMATE_MAX} THEN 'blitz' \
         ELSE 'rapid' END"
    )
}

/// Probability that `rating` scores against `opponent` (logistic Elo curve).
pub fn expected_score(rating: i32, opponent: i32) -> f64 {
    1.0 / (1.0 + 10_f64.powf((opponent - rating) as f64 / 400.0))
}

fn k_factor(rating: i32, games_played: i32) -> f64 {
    if rating >= MASTER_RATING {
        K_MASTER
    } else if games_played < PROVISIONAL_GAMES {
        K_PROVISIONAL
    } else {
        K_ESTABLISHED
    }
}

/// New rating after `score` (1.0 win, 0.5 draw, 0.0 loss) against `opponent`.
pub fn new_rating(rating: i32, games_played: i32, opponent: i32, score: f64) -> i32 {
    let k = k_factor(rating, games_played);
    let updated = (rating as f64) + k * (score - expected_score(rating, opponent));
    (updated.round() as i32).max(0)
}

/// Apply the Elo exchange for a finished, rated game.
///
/// Called inside the transaction that ends the game; a no-op otherwise.
pub async fn settle(conn: &mut PgConnection, game: &Game) -> Result<(), ApiError> {
    if game.status != GameStatus::Finished || !game.rated {
        return Ok(());
    }
    let Some(result) = game.result else {
        return Ok(());
    };
    // Both seats must be occupied for a rating to be meaningful.
    let Some(black_id) = game.black_player_id else {
        return Ok(());
    };
    let white_id = game.white_player_id;

    let category = category_for(game.initial_time_ms, game.increment_ms);
    let white = repo::get_or_create(&mut *conn, white_id, category).await?;
    let black = repo::get_or_create(&mut *conn, black_id, category).await?;

    let score_white = match result.winner {
        None => 0.5,
        Some(game_core::Color::White) => 1.0,
        Some(_) => 0.0,
    };
    let white_after = new_rating(white.rating, white.games_played, black.rating, score_white);
    let black_after = new_rating(
        black.rating,
        black.games_played,
        white.rating,
        1.0 - score_white,
    );

    repo::set_rating(&mut *conn, white_id, category, white_after).await?;
    repo::set_rating(&mut *conn, black_id, category, black_after).await?;
    repo::insert_history(&mut *conn, white_id, game.id, white.rating, white_after).await?;
    repo::insert_history(&mut *conn, black_id, game.id, black.rating, black_after).await?;

    tracing::info!(
        game = %game.id,
        category,
        white = %white_id,
        white_after,
        black = %black_id,
        black_after,
        "ratings settled"
    );
    Ok(())
}

/// Assemble a profile DTO, filling in defaults for categories with no row.
pub fn profile_dto(user: &protocol::UserDto, ratings: &HashMap<String, (i32, i32)>) -> ProfileDto {
    let ratings = CATEGORIES
        .iter()
        .map(|category| {
            let (rating, games_played) = ratings
                .get(*category)
                .copied()
                .unwrap_or((DEFAULT_RATING, 0));
            RatingDto {
                category: (*category).to_string(),
                rating,
                games_played,
            }
        })
        .collect();
    ProfileDto {
        user: user.clone(),
        ratings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_follow_the_usual_convention() {
        assert_eq!(category_for(60_000, 0), "bullet"); // 1+0
        assert_eq!(category_for(120_000, 1_000), "bullet"); // 2+1
        assert_eq!(category_for(180_000, 2_000), "blitz"); // 3+2
        assert_eq!(category_for(300_000, 0), "blitz"); // 5+0
        assert_eq!(category_for(600_000, 0), "rapid"); // 10+0
        assert_eq!(category_for(900_000, 10_000), "rapid"); // 15+10
    }

    #[test]
    fn category_sql_carries_the_same_thresholds() {
        let sql = category_sql("g.initial_time_ms", "g.increment_ms");
        // The SQL twin is a mechanical echo of `category_for`; the least we can
        // check without a database is that it embeds the shared constants and
        // labels, in the same order the Rust thresholds decide.
        assert!(sql.contains(&BULLET_ESTIMATE_MAX.to_string()), "{sql}");
        assert!(sql.contains(&BLITZ_ESTIMATE_MAX.to_string()), "{sql}");
        assert!(
            sql.contains("'bullet'") && sql.contains("'blitz'") && sql.contains("'rapid'"),
            "{sql}"
        );
        let bullet = sql.find("THEN 'bullet'").expect("bullet branch");
        let blitz = sql.find("THEN 'blitz'").expect("blitz branch");
        let rapid = sql.find("ELSE 'rapid'").expect("rapid branch");
        assert!(bullet < blitz && blitz < rapid, "branch order: {sql}");
        assert!(
            sql.contains("g.initial_time_ms + 40 * g.increment_ms"),
            "{sql}"
        );
    }

    #[test]
    fn equal_players_exchange_symmetrically() {
        assert!((expected_score(1500, 1500) - 0.5).abs() < 1e-9);
        // A win against an equal opponent gains exactly the K factor.
        assert_eq!(new_rating(1500, 100, 1500, 1.0), 1508);
        assert_eq!(new_rating(1500, 100, 1500, 0.0), 1492);
        // A draw between equals does not move the rating.
        assert_eq!(new_rating(1500, 100, 1500, 0.5), 1500);
    }

    #[test]
    fn provisional_players_move_faster() {
        assert_eq!(new_rating(1500, 0, 1500, 1.0), 1516);
        assert_eq!(new_rating(1500, 29, 1500, 1.0), 1516);
        assert_eq!(new_rating(1500, 30, 1500, 1.0), 1508);
    }

    #[test]
    fn upset_gains_more_than_expected_win() {
        let upset = new_rating(1200, 100, 1800, 1.0);
        let expected_win = new_rating(1200, 100, 1000, 1.0);
        assert!(upset > 1200 + 10, "beating a strong player must pay off");
        assert!(expected_win < 1200 + 10);
    }
}
