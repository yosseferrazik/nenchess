//! Test support: data factories and board scenarios shared by unit,
//! integration and end-to-end tests.

use game_core::{starting_fen, validate_and_apply, MoveInput};
use uuid::Uuid;

/// A ready-to-register test user.
#[derive(Debug, Clone)]
pub struct TestUser {
    pub username: String,
    pub password: String,
}

impl TestUser {
    pub fn new(prefix: &str) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let suffix: String = suffix.chars().take(8).collect();
        Self {
            username: format!("{prefix}{suffix}"),
            password: "correct-horse-battery".to_string(),
        }
    }
}

pub fn white_player() -> TestUser {
    TestUser::new("white")
}

pub fn black_player() -> TestUser {
    TestUser::new("black")
}

/// Apply a UCI move sequence to a position, panicking on illegal moves.
pub fn play(fen: &str, ucis: &[&str]) -> String {
    let mut current = fen.to_string();
    for uci in ucis {
        let mv = MoveInput::from_uci(uci).expect("test move must parse");
        current = validate_and_apply(&current, &mv)
            .expect("test move must be legal")
            .0;
    }
    current
}

/// Fool's mate: black wins, white is checkmated.
pub fn fools_mate() -> (String, &'static [&'static str]) {
    const MOVES: &[&str] = &["f2f3", "e7e5", "g2g4", "d8h4"];
    (play(starting_fen(), MOVES), MOVES)
}

/// A known stalemate position.
pub fn stalemate_fen() -> &'static str {
    "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"
}
