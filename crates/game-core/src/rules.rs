//! Chess rules: legal move validation, position status and FEN handling.
//!
//! `cozy-chess` (MIT) is used as the move generator. Its types stay behind
//! this module's API — callers only ever see `game-core` types.

use std::str::FromStr;

use cozy_chess::{Board, Move as EngineMove};
use thiserror::Error;

use crate::types::{Color, PieceKind, Square};

/// The standard starting position.
pub const STARTING_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

pub fn starting_fen() -> &'static str {
    STARTING_FEN
}

/// A move as sent by a client: origin, destination and optional promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveInput {
    pub from: Square,
    pub to: Square,
    pub promotion: Option<PieceKind>,
}

impl MoveInput {
    /// UCI notation, e.g. `e2e4` or `e7e8q`.
    pub fn to_uci(&self) -> String {
        let mut uci = format!("{}{}", self.from, self.to);
        if let Some(piece) = self.promotion {
            uci.push(piece.uci_letter());
        }
        uci
    }

    /// Parse UCI notation (`e2e4`, `e7e8q`).
    pub fn from_uci(uci: &str) -> Result<Self, MoveError> {
        let bytes = uci.as_bytes();
        if bytes.len() != 4 && bytes.len() != 5 {
            return Err(MoveError::InvalidUci(uci.to_string()));
        }
        let from = std::str::from_utf8(&bytes[0..2])
            .ok()
            .and_then(|s| s.parse::<Square>().ok())
            .ok_or_else(|| MoveError::InvalidUci(uci.to_string()))?;
        let to = std::str::from_utf8(&bytes[2..4])
            .ok()
            .and_then(|s| s.parse::<Square>().ok())
            .ok_or_else(|| MoveError::InvalidUci(uci.to_string()))?;
        let promotion = match bytes.len() {
            5 => Some(
                PieceKind::from_uci_letter(bytes[4] as char)
                    .ok_or_else(|| MoveError::InvalidUci(uci.to_string()))?,
            ),
            _ => None,
        };
        Ok(MoveInput {
            from,
            to,
            promotion,
        })
    }
}

/// Terminal state of a game, derived from the position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    /// The game continues.
    InProgress,
    /// Checkmate: the side to move is mated.
    Checkmate { loser: Color },
    /// Stalemate: no legal moves, not in check.
    Stalemate,
    /// Drawn by rule (currently: fifty-move rule; repetition and insufficient
    /// material are tracked as TODO in the roadmap).
    Drawn,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MoveError {
    #[error("invalid FEN position: {0}")]
    InvalidFen(String),
    #[error("invalid UCI move: {0}")]
    InvalidUci(String),
    #[error("illegal move: {0}")]
    IllegalMove(String),
}

/// Validate and apply a move, returning the resulting FEN and status.
pub fn validate_and_apply(fen: &str, mv: &MoveInput) -> Result<(String, GameStatus), MoveError> {
    let mut board = parse_fen(fen)?;
    let uci = mv.to_uci();
    let engine_move =
        EngineMove::from_str(&uci).map_err(|_| MoveError::IllegalMove(uci.clone()))?;
    if !board.is_legal(engine_move) {
        return Err(MoveError::IllegalMove(uci));
    }
    board
        .try_play(engine_move)
        .map_err(|_| MoveError::IllegalMove(uci))?;
    let status = position_status(&board)?;
    Ok((board.to_string(), status))
}

/// Status of the position described by `fen`.
pub fn status(fen: &str) -> Result<GameStatus, MoveError> {
    let board = parse_fen(fen)?;
    position_status(&board)
}

/// All legal moves of the position, in UCI notation.
pub fn legal_moves(fen: &str) -> Result<Vec<String>, MoveError> {
    let board = parse_fen(fen)?;
    let mut moves = Vec::new();
    board.generate_moves(|piece_moves| {
        for mv in piece_moves {
            moves.push(mv.to_string());
        }
        false
    });
    Ok(moves)
}

fn parse_fen(fen: &str) -> Result<Board, MoveError> {
    Board::from_fen(fen, false).map_err(|_| MoveError::InvalidFen(fen.to_string()))
}

fn position_status(board: &Board) -> Result<GameStatus, MoveError> {
    use cozy_chess::GameStatus as EngineStatus;
    Ok(match board.status() {
        EngineStatus::Ongoing => GameStatus::InProgress,
        EngineStatus::Won => GameStatus::Checkmate {
            loser: map_color(board.side_to_move()),
        },
        EngineStatus::Drawn => {
            let has_moves = {
                let mut any = false;
                board.generate_moves(|piece_moves| {
                    if !piece_moves.is_empty() {
                        any = true;
                        true
                    } else {
                        false
                    }
                });
                any
            };
            if has_moves {
                GameStatus::Drawn
            } else {
                GameStatus::Stalemate
            }
        }
    })
}

fn map_color(color: cozy_chess::Color) -> Color {
    match color {
        cozy_chess::Color::White => Color::White,
        cozy_chess::Color::Black => Color::Black,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(fen: &str, ucis: &[&str]) -> String {
        let mut current = fen.to_string();
        for uci in ucis {
            let mv = MoveInput::from_uci(uci).unwrap();
            current = validate_and_apply(&current, &mv).unwrap().0;
        }
        current
    }

    #[test]
    fn starting_position_has_twenty_moves() {
        assert_eq!(legal_moves(starting_fen()).unwrap().len(), 20);
        assert_eq!(status(starting_fen()).unwrap(), GameStatus::InProgress);
    }

    #[test]
    fn rejects_illegal_moves() {
        let mv = MoveInput::from_uci("e2e5").unwrap();
        assert!(matches!(
            validate_and_apply(starting_fen(), &mv),
            Err(MoveError::IllegalMove(_))
        ));
        // Moving the opponent's piece is not allowed either.
        let mv = MoveInput::from_uci("e7e5").unwrap();
        assert!(validate_and_apply(starting_fen(), &mv).is_err());
    }

    #[test]
    fn detects_checkmate() {
        let fen = play(starting_fen(), &["f2f3", "e7e5", "g2g4", "d8h4"]);
        assert_eq!(
            status(&fen).unwrap(),
            GameStatus::Checkmate {
                loser: Color::White
            }
        );
    }

    #[test]
    fn detects_stalemate() {
        let fen = "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1";
        assert_eq!(status(fen).unwrap(), GameStatus::Stalemate);
    }

    #[test]
    fn handles_promotion() {
        // White pawn on a7 promotes on a8; the black king on h7 stays put.
        let fen = "8/P6k/8/8/8/8/7K/8 w - - 0 1";
        let mv = MoveInput::from_uci("a7a8q").unwrap();
        let (after, _) = validate_and_apply(fen, &mv).unwrap();
        assert!(after.starts_with("Q7/"), "unexpected FEN: {after}");
        assert_eq!(mv.promotion, Some(PieceKind::Queen));
    }

    #[test]
    fn uci_roundtrip() {
        let mv = MoveInput::from_uci("e7e8n").unwrap();
        assert_eq!(mv.to_uci(), "e7e8n");
        assert_eq!(mv.promotion, Some(PieceKind::Knight));
        assert!(MoveInput::from_uci("xyz").is_err());
    }
}
