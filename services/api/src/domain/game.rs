//! The game aggregate: the single most important piece of state in the
//! platform. All transitions are explicit functions so they can be unit
//! tested without a database, a socket or a framework.

use game_core::{
    starting_fen, validate_and_apply, Clock, Color, GameStatus as RulesStatus, MoveInput,
    PieceKind, Square,
};
use uuid::Uuid;

/// Lifecycle of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    /// Created; waiting for the second player.
    Waiting,
    /// In progress; clocks running.
    Active,
    /// Terminal; `result` is always set.
    Finished,
}

impl GameStatus {
    pub fn as_db(self) -> &'static str {
        match self {
            GameStatus::Waiting => "waiting",
            GameStatus::Active => "active",
            GameStatus::Finished => "finished",
        }
    }

    pub fn from_db(raw: &str) -> Option<Self> {
        match raw {
            "waiting" => Some(GameStatus::Waiting),
            "active" => Some(GameStatus::Active),
            "finished" => Some(GameStatus::Finished),
            _ => None,
        }
    }
}

/// Why a game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultReason {
    Checkmate,
    Stalemate,
    Resignation,
    Flag,
    FiftyMove,
    Abandonment,
}

impl ResultReason {
    pub fn as_db(self) -> &'static str {
        match self {
            ResultReason::Checkmate => "checkmate",
            ResultReason::Stalemate => "stalemate",
            ResultReason::Resignation => "resignation",
            ResultReason::Flag => "flag",
            ResultReason::FiftyMove => "fifty_move",
            ResultReason::Abandonment => "abandonment",
        }
    }

    pub fn from_db(raw: &str) -> Option<Self> {
        match raw {
            "checkmate" => Some(ResultReason::Checkmate),
            "stalemate" => Some(ResultReason::Stalemate),
            "resignation" => Some(ResultReason::Resignation),
            "flag" => Some(ResultReason::Flag),
            "fifty_move" => Some(ResultReason::FiftyMove),
            "abandonment" => Some(ResultReason::Abandonment),
            _ => None,
        }
    }
}

/// Final result. `winner: None` means a draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameResult {
    pub winner: Option<Color>,
    pub reason: ResultReason,
}

/// Immutable move record appended to `game_moves`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveRecord {
    pub move_number: i32,
    pub from: Square,
    pub to: Square,
    pub promotion: Option<PieceKind>,
    pub uci: String,
}

/// The authoritative game state. Persisted as a row in `games`; moves are
/// appended to `game_moves` and every transition bumps `version`.
#[derive(Debug, Clone)]
pub struct Game {
    pub id: Uuid,
    pub white_player_id: Uuid,
    pub black_player_id: Option<Uuid>,
    pub status: GameStatus,
    pub fen: String,
    pub turn: Color,
    pub version: i64,
    pub move_count: i32,
    pub initial_time_ms: i64,
    pub increment_ms: i64,
    pub rated: bool,
    pub clock: Clock,
    pub result: Option<GameResult>,
}

impl Game {
    /// Create a new game waiting for an opponent. The clock is (re)anchored
    /// when the second player joins.
    pub fn new(
        id: Uuid,
        white_player_id: Uuid,
        initial_time_ms: i64,
        increment_ms: i64,
        rated: bool,
        now_ms: i64,
    ) -> Self {
        Self {
            id,
            white_player_id,
            black_player_id: None,
            status: GameStatus::Waiting,
            fen: starting_fen().to_string(),
            turn: Color::White,
            version: 0,
            move_count: 0,
            initial_time_ms,
            increment_ms,
            rated,
            clock: Clock::new(initial_time_ms, increment_ms, Color::White, now_ms),
            result: None,
        }
    }

    /// Colour of `user` in this game, if they are a participant.
    pub fn participant_color(&self, user: Uuid) -> Option<Color> {
        if user == self.white_player_id {
            Some(Color::White)
        } else if self.black_player_id == Some(user) {
            Some(Color::Black)
        } else {
            None
        }
    }

    fn require_active(&self) -> Result<(), DomainError> {
        match self.status {
            GameStatus::Active => Ok(()),
            GameStatus::Waiting => Err(DomainError::AwaitingOpponent),
            GameStatus::Finished => Err(DomainError::GameOver),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("the game has not started yet")]
    AwaitingOpponent,
    #[error("the game is already finished")]
    GameOver,
    #[error("stale state version: expected {expected}, server has {actual}")]
    VersionMismatch { expected: i64, actual: i64 },
    #[error("you are not a participant of this game")]
    NotParticipant,
    #[error("it is not your turn")]
    NotYourTurn,
    #[error("the game already has two players")]
    GameFull,
    #[error("you are already seated in this game")]
    AlreadySeated,
    #[error("illegal move: {0}")]
    IllegalMove(String),
}

/// Result of attempting a move.
pub enum MoveOutcome {
    /// Move validated and applied. `ended` is true when it produced a
    /// terminal position (checkmate/stalemate/draw).
    Applied {
        game: Game,
        applied: MoveRecord,
        fen_after: String,
        ended: bool,
    },
    /// The mover's flag had already fallen at server receipt time: the game
    /// ends without applying the move. Deterministic policy: the flag falls
    /// when `now >= turn_started + remaining`, evaluated with the single
    /// server clock under the per-game lock.
    FlagExpired { game: Game },
}

/// Validate and apply a move.
///
/// Checks performed in order: lifecycle → optimistic version →
/// participation → side to move → flag → legality.
pub fn apply_move(
    game: Game,
    mover: Uuid,
    expected_version: i64,
    mv: &MoveInput,
    now_ms: i64,
) -> Result<MoveOutcome, DomainError> {
    game.require_active()?;
    if game.version != expected_version {
        return Err(DomainError::VersionMismatch {
            expected: expected_version,
            actual: game.version,
        });
    }
    let mover_color = game
        .participant_color(mover)
        .ok_or(DomainError::NotParticipant)?;
    if game.turn != mover_color {
        return Err(DomainError::NotYourTurn);
    }

    // Deterministic expiry-vs-move race: the server timestamp of the command
    // decides. Equal to or after the expiry instant means the flag has fallen.
    if game.clock.flag_fallen(now_ms) {
        let mut g = game;
        match mover_color {
            Color::White => g.clock.white_ms = 0,
            Color::Black => g.clock.black_ms = 0,
        }
        g.version += 1;
        g.status = GameStatus::Finished;
        g.result = Some(GameResult {
            winner: Some(mover_color.opposite()),
            reason: ResultReason::Flag,
        });
        return Ok(MoveOutcome::FlagExpired { game: g });
    }

    let (fen_after, rules_status) = validate_and_apply(&game.fen, mv)
        .map_err(|err| DomainError::IllegalMove(err.to_string()))?;

    let mut g = game;
    g.version += 1;
    g.move_count += 1;
    let move_number = g.move_count;
    g.clock.apply_move(mover_color, now_ms);
    g.turn = g.clock.turn;
    g.fen = fen_after.clone();

    let ended = match rules_status {
        RulesStatus::InProgress => false,
        RulesStatus::Checkmate { loser } => {
            g.result = Some(GameResult {
                winner: Some(loser.opposite()),
                reason: ResultReason::Checkmate,
            });
            true
        }
        RulesStatus::Stalemate => {
            g.result = Some(GameResult {
                winner: None,
                reason: ResultReason::Stalemate,
            });
            true
        }
        RulesStatus::Drawn => {
            g.result = Some(GameResult {
                winner: None,
                reason: ResultReason::FiftyMove,
            });
            true
        }
    };
    if ended {
        g.status = GameStatus::Finished;
    }

    let applied = MoveRecord {
        move_number,
        from: mv.from,
        to: mv.to,
        promotion: mv.promotion,
        uci: mv.to_uci(),
    };
    Ok(MoveOutcome::Applied {
        game: g,
        applied,
        fen_after,
        ended,
    })
}

/// Resign: allowed for any participant while the game is active. No expected
/// version — under the per-game lock the decision cannot be raced, and
/// resigning is valid regardless of how many moves happened meanwhile.
pub fn resign(game: Game, resigner: Uuid, now_ms: i64) -> Result<Game, DomainError> {
    game.require_active()?;
    let resigner_color = game
        .participant_color(resigner)
        .ok_or(DomainError::NotParticipant)?;
    let mut g = game;
    // Charge the running clock up to the resignation instant so the final
    // snapshot shows truthful remaining times.
    let elapsed = g.clock.elapsed_ms(now_ms);
    let rest = match g.clock.turn {
        Color::White => &mut g.clock.white_ms,
        Color::Black => &mut g.clock.black_ms,
    };
    *rest = (*rest - elapsed).max(0);
    g.version += 1;
    g.status = GameStatus::Finished;
    g.result = Some(GameResult {
        winner: Some(resigner_color.opposite()),
        reason: ResultReason::Resignation,
    });
    Ok(g)
}

/// Second player joins: the game becomes active and the clocks are anchored
/// at `now_ms`.
pub fn join(game: Game, joiner: Uuid, now_ms: i64) -> Result<Game, DomainError> {
    match game.status {
        GameStatus::Waiting => {}
        GameStatus::Active | GameStatus::Finished => return Err(DomainError::GameFull),
    }
    if game.white_player_id == joiner {
        return Err(DomainError::AlreadySeated);
    }
    if game.black_player_id.is_some() {
        return Err(DomainError::GameFull);
    }
    let mut g = game;
    g.black_player_id = Some(joiner);
    g.status = GameStatus::Active;
    g.version += 1;
    g.turn = Color::White;
    g.clock = Clock::new(g.initial_time_ms, g.increment_ms, Color::White, now_ms);
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_core::starting_fen;

    fn active_game(now: i64) -> (Game, Uuid, Uuid) {
        let white = Uuid::new_v4();
        let black = Uuid::new_v4();
        let created = Game::new(Uuid::new_v4(), white, 600_000, 0, false, now);
        let game = join(created, black, now).expect("join");
        (game, white, black)
    }

    fn mv(uci: &str) -> MoveInput {
        MoveInput::from_uci(uci).expect("parse")
    }

    #[test]
    fn starts_waiting_and_becomes_active_on_join() {
        let white = Uuid::new_v4();
        let created = Game::new(Uuid::new_v4(), white, 600_000, 2_000, true, 1_000);
        assert_eq!(created.status, GameStatus::Waiting);
        assert_eq!(created.version, 0);
        assert_eq!(created.fen, starting_fen());

        let black = Uuid::new_v4();
        let joined = join(created, black, 5_000).expect("join");
        assert_eq!(joined.status, GameStatus::Active);
        assert_eq!(joined.version, 1);
        assert_eq!(joined.clock.turn_started_ms, 5_000);
        assert!(join(joined.clone(), Uuid::new_v4(), 6_000).is_err());
        assert!(join(joined.clone(), white, 6_000).is_err());
    }

    #[test]
    fn applies_moves_and_bumps_version() {
        let now = 1_000;
        let (game, white, _black) = active_game(now);
        let outcome =
            apply_move(game.clone(), white, 1, &mv("e2e4"), now + 100).expect("legal move");
        match outcome {
            MoveOutcome::Applied {
                game,
                applied,
                ended,
                ..
            } => {
                assert!(!ended);
                assert_eq!(game.version, 2);
                assert_eq!(game.move_count, 1);
                assert_eq!(applied.uci, "e2e4");
                assert_eq!(game.turn, Color::Black);
                assert_eq!(game.result, None);
            }
            _ => panic!("expected Applied"),
        }
    }

    #[test]
    fn rejects_wrong_version_turn_and_strangers() {
        let now = 1_000;
        let (game, white, black) = active_game(now);

        // Stale version.
        assert!(matches!(
            apply_move(game.clone(), white, 99, &mv("e2e4"), now),
            Err(DomainError::VersionMismatch { .. })
        ));
        // Wrong turn (white's move with correct version).
        assert!(matches!(
            apply_move(game.clone(), black, 1, &mv("e7e5"), now),
            Err(DomainError::NotYourTurn)
        ));
        // Stranger.
        assert!(matches!(
            apply_move(game.clone(), Uuid::new_v4(), 1, &mv("e2e4"), now),
            Err(DomainError::NotParticipant)
        ));
        // Illegal move.
        assert!(matches!(
            apply_move(game.clone(), white, 1, &mv("e2e5"), now),
            Err(DomainError::IllegalMove(_))
        ));
    }

    #[test]
    fn flag_expiry_ends_the_game_deterministically() {
        let now = 1_000;
        let (game, white, _black) = active_game(now);
        // Policy: the flag falls exactly at `turn_started + remaining`.
        let expiry = game.clock.flag_falls_at_ms();
        assert_eq!(expiry, game.clock.turn_started_ms + 600_000);
        let outcome =
            apply_move(game, white, 1, &mv("e2e4"), expiry - 1).expect("one ms before expiry");
        assert!(matches!(outcome, MoveOutcome::Applied { ended: false, .. }));

        let (game2, white2, _b) = active_game(now);
        let outcome = apply_move(game2, white2, 1, &mv("e2e4"), expiry).expect("at expiry");
        match outcome {
            MoveOutcome::FlagExpired { game } => {
                assert_eq!(game.status, GameStatus::Finished);
                assert_eq!(
                    game.result,
                    Some(GameResult {
                        winner: Some(Color::Black),
                        reason: ResultReason::Flag
                    })
                );
                assert_eq!(game.move_count, 0);
                assert_eq!(game.clock.white_ms, 0);
                assert_eq!(game.version, 2);
            }
            _ => panic!("expected FlagExpired"),
        }
    }

    #[test]
    fn checkmate_ends_the_game() {
        let now = 1_000;
        let (mut game, white, black) = active_game(now);
        let sequence = ["f2f3", "e7e5", "g2g4", "d8h4"];
        let mut mover = white;
        let mut t = now;
        for (i, uci) in sequence.iter().enumerate() {
            let expected = i as i64 + 1;
            let outcome = apply_move(game, mover, expected, &mv(uci), t + 10).expect("move");
            mover = if mover == white { black } else { white };
            match outcome {
                MoveOutcome::Applied { game: g, ended, .. } => {
                    if i == sequence.len() - 1 {
                        assert!(ended);
                        assert_eq!(g.status, GameStatus::Finished);
                        assert_eq!(
                            g.result,
                            Some(GameResult {
                                winner: Some(Color::Black),
                                reason: ResultReason::Checkmate
                            })
                        );
                    } else {
                        assert!(!ended);
                    }
                    game = g;
                }
                _ => panic!("expected Applied"),
            }
            t += 10;
        }
        // Once finished, further moves are rejected.
        assert!(matches!(
            apply_move(game, white, 99, &mv("a2a3"), t),
            Err(DomainError::GameOver)
        ));
    }

    #[test]
    fn resign_charges_running_clock_and_ends() {
        let now = 1_000;
        let (game, white, _black) = active_game(now);
        let finished = resign(game, white, now + 30_000).expect("resign");
        assert_eq!(finished.status, GameStatus::Finished);
        assert_eq!(
            finished.result,
            Some(GameResult {
                winner: Some(Color::Black),
                reason: ResultReason::Resignation
            })
        );
        // White ran 30s: 600_000 - 30_000 = 570_000.
        assert_eq!(finished.clock.white_ms, 570_000);
        assert_eq!(finished.version, 2);

        // Strangers cannot resign.
        let (game2, _w, _b) = active_game(now);
        assert!(matches!(
            resign(game2, Uuid::new_v4(), now),
            Err(DomainError::NotParticipant)
        ));
    }

    #[test]
    fn status_and_reason_roundtrip_through_db_strings() {
        assert_eq!(GameStatus::from_db("active"), Some(GameStatus::Active));
        assert_eq!(GameStatus::from_db("nope"), None);
        assert_eq!(GameStatus::Finished.as_db(), "finished");
        assert_eq!(
            ResultReason::from_db("fifty_move"),
            Some(ResultReason::FiftyMove)
        );
        assert_eq!(ResultReason::Flag.as_db(), "flag");
        assert_eq!(ResultReason::from_db("nope"), None);
    }
}
