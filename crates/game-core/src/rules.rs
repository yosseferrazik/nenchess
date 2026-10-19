//! Chess rules: legal move validation, position status and FEN handling.
//!
//! `cozy-chess` (MIT) is used as the move generator. Its types stay behind
//! this module's API — callers only ever see `game-core` types.

use std::str::FromStr;

use cozy_chess::{BitBoard, Board, File, Move as EngineMove, Piece, Square as EngineSquare};
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

/// Why a position is drawn by rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawKind {
    /// Fifty moves have elapsed without a pawn move or a capture (also covers
    /// the automatic seventy-five-move rule, since it is a superset).
    FiftyMove,
    /// The same position (pieces, side to move, castling rights and en-passant
    /// availability) has occurred at least three times.
    ThreefoldRepetition,
    /// Neither side can possibly deliver checkmate (dead position).
    InsufficientMaterial,
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
    /// Drawn by rule.
    Drawn(DrawKind),
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
    let engine_move = to_engine_move(&board, mv)?;
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

/// The canonical (standard-UCI) spelling of a move that is legal in `fen`.
///
/// A nonstandard client may spell castling as king-takes-rook; records,
/// views and exports always speak king-moves-two, so downstream consumers
/// never need to know the engine's representation.
pub fn normalize(fen: &str, mv: &MoveInput) -> Result<MoveInput, MoveError> {
    let board = parse_fen(fen)?;
    let engine_move = to_engine_move(&board, mv)?;
    if !board.is_legal(engine_move) {
        return Err(MoveError::IllegalMove(mv.to_uci()));
    }
    MoveInput::from_uci(&engine_move_to_uci(&board, engine_move))
}

/// All legal moves of the position, in (standard) UCI notation. Castling is
/// spelled the way clients and every UI expect it: the king moving two
/// squares (`e1g1`), never king-takes-rook.
pub fn legal_moves(fen: &str) -> Result<Vec<String>, MoveError> {
    let board = parse_fen(fen)?;
    let mut moves = Vec::new();
    board.generate_moves(|piece_moves| {
        for mv in piece_moves {
            moves.push(engine_move_to_uci(&board, mv));
        }
        false
    });
    Ok(moves)
}

/// `cozy-chess` expresses castling as the king moving onto its rook
/// (chess960-compatible); standard UCI — and every chess UI — expresses it
/// as the king moving two squares. Accept both spellings so clients never
/// need to know the engine's representation.
pub(crate) fn to_engine_move(board: &Board, mv: &MoveInput) -> Result<EngineMove, MoveError> {
    let uci = mv.to_uci();
    let direct = EngineMove::from_str(&uci).map_err(|_| MoveError::IllegalMove(uci.clone()))?;
    // Castling is the only king move that changes files (by two of them).
    let is_king = board.piece_on(direct.from) == Some(Piece::King);
    if is_king && direct.from.file() != direct.to.file() {
        let rook_file = if (direct.to.file() as usize) > (direct.from.file() as usize) {
            File::H
        } else {
            File::A
        };
        return Ok(EngineMove {
            from: direct.from,
            to: EngineSquare::new(rook_file, direct.from.rank()),
            promotion: None,
        });
    }
    Ok(direct)
}

/// The standard UCI spelling of an engine move (the inverse of
/// [`to_engine_move`]).
pub(crate) fn engine_move_to_uci(board: &Board, mv: EngineMove) -> String {
    let is_king = board.piece_on(mv.from) == Some(Piece::King);
    if is_king && (mv.to.file() as usize).abs_diff(mv.from.file() as usize) > 1 {
        let dest_file = if (mv.to.file() as usize) > (mv.from.file() as usize) {
            File::G
        } else {
            File::C
        };
        return format!(
            "{}{}",
            mv.from,
            EngineSquare::new(dest_file, mv.from.rank())
        );
    }
    mv.to_string()
}

/// A Zobrist hash that identifies a position for repetition detection: two
/// positions share it if and only if they have the same pieces, side to move,
/// castling rights and en-passant square.
///
/// It is deliberately *conservative*: including the en-passant square even when
/// no capture is available can only make two truly equal positions look
/// different (a missed repetition), never make two different positions look
/// equal (a false draw).
pub fn position_hash(fen: &str) -> Result<u64, MoveError> {
    Ok(parse_fen(fen)?.hash())
}

/// Whether the position is dead: no sequence of legal moves can deliver
/// checkmate. Covers bare kings, king + one minor piece, and king + bishop
/// versus king + bishop with both bishops on the same colour complex.
pub fn insufficient_material(fen: &str) -> Result<bool, MoveError> {
    let board = parse_fen(fen)?;
    Ok(insufficient_material_board(&board))
}

fn parse_fen(fen: &str) -> Result<Board, MoveError> {
    Board::from_fen(fen, false).map_err(|_| MoveError::InvalidFen(fen.to_string()))
}

fn position_status(board: &Board) -> Result<GameStatus, MoveError> {
    // Checkmate and stalemate take precedence over every draw rule: a move that
    // mates ends the game even if the half-move counter has also run out.
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
    if !has_moves {
        return Ok(if board.checkers().is_empty() {
            GameStatus::Stalemate
        } else {
            GameStatus::Checkmate {
                loser: map_color(board.side_to_move()),
            }
        });
    }
    if board.halfmove_clock() >= 100 {
        return Ok(GameStatus::Drawn(DrawKind::FiftyMove));
    }
    if insufficient_material_board(board) {
        return Ok(GameStatus::Drawn(DrawKind::InsufficientMaterial));
    }
    Ok(GameStatus::InProgress)
}

/// Dead-position test on an already parsed board (see [`insufficient_material`]).
fn insufficient_material_board(board: &Board) -> bool {
    use cozy_chess::Color as EngineColor;

    // A pawn, rook or queen can always, in principle, deliver mate.
    if !board.pieces(Piece::Pawn).is_empty()
        || !board.pieces(Piece::Rook).is_empty()
        || !board.pieces(Piece::Queen).is_empty()
    {
        return false;
    }

    let white_bishops = board.colored_pieces(EngineColor::White, Piece::Bishop);
    let black_bishops = board.colored_pieces(EngineColor::Black, Piece::Bishop);
    let knights = board
        .colored_pieces(EngineColor::White, Piece::Knight)
        .len()
        + board
            .colored_pieces(EngineColor::Black, Piece::Knight)
            .len();

    match (white_bishops.len() + black_bishops.len(), knights) {
        // Bare kings.
        (0, 0) => true,
        // King and a single minor piece cannot mate.
        (1, 0) | (0, 1) => true,
        // King + bishop versus king + bishop: drawn only when both bishops
        // travel on the same colour complex.
        (2, 0) => match (only_square(white_bishops), only_square(black_bishops)) {
            (Some(a), Some(b)) => square_is_light(a) == square_is_light(b),
            _ => false,
        },
        _ => false,
    }
}

fn only_square(set: BitBoard) -> Option<EngineSquare> {
    let mut squares = set.into_iter();
    match (squares.next(), squares.next()) {
        (Some(square), None) => Some(square),
        _ => None,
    }
}

/// `true` for a light square, `false` for a dark one. `Square` is ordered
/// rank-major (a1 = 0), so `file + rank` parity gives the square colour.
fn square_is_light(square: EngineSquare) -> bool {
    let index = square as usize;
    ((index & 7) + (index >> 3)) % 2 == 1
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
    fn detects_fifty_move_draw() {
        // Half-move clock already at 100 with plenty of material on the board.
        let fen = "7k/8/8/8/8/8/8/1K4R1 w - - 100 51";
        assert_eq!(status(fen).unwrap(), GameStatus::Drawn(DrawKind::FiftyMove));
    }

    #[test]
    fn detects_insufficient_material() {
        assert!(insufficient_material("7k/8/8/8/8/8/8/K7 w - - 0 1").unwrap());
        assert!(insufficient_material("7k/8/8/8/8/8/8/KB6 w - - 0 1").unwrap());
        assert!(insufficient_material("7k/8/8/8/8/8/8/KN6 w - - 0 1").unwrap());
        // Same-colour bishops: dead. b1 is dark, g8 is dark.
        assert!(insufficient_material("6bk/8/8/8/8/8/8/1B5K w - - 0 1").unwrap());
        // Opposite-colour bishops: still possible (with help) to mate.
        assert!(!insufficient_material("7k/6b1/8/8/8/8/8/1B5K w - - 0 1").unwrap());
        // A lone rook is enough material.
        assert!(!insufficient_material("7k/8/8/8/8/8/8/1K4R1 w - - 0 1").unwrap());
        assert_eq!(
            status("7k/8/8/8/8/8/8/KB6 w - - 0 1").unwrap(),
            GameStatus::Drawn(DrawKind::InsufficientMaterial)
        );
    }

    #[test]
    fn position_hash_ignores_counters_but_tracks_state() {
        // Same pieces, side to move and rights; only the counters differ.
        let a = "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1";
        let b = "8/8/8/4k3/8/8/4P3/4K3 w - - 7 40";
        assert_eq!(position_hash(a).unwrap(), position_hash(b).unwrap());
        // Different side to move is a different position.
        let c = "8/8/8/4k3/8/8/4P3/4K3 b - - 0 1";
        assert_ne!(position_hash(a).unwrap(), position_hash(c).unwrap());
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

    #[test]
    fn castling_is_accepted_in_both_spellings() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        // Standard UCI — what every UI sends: the king moves two squares.
        let (after, status) =
            validate_and_apply(fen, &MoveInput::from_uci("e1g1").unwrap()).unwrap();
        assert_eq!(status, GameStatus::InProgress);
        assert!(
            after.starts_with("r3k2r/8/8/8/8/8/8/R4RK1"),
            "unexpected FEN: {after}"
        );
        // The engine's own spelling (king takes rook) is accepted too.
        let (after, _) = validate_and_apply(fen, &MoveInput::from_uci("e1h1").unwrap()).unwrap();
        assert!(
            after.starts_with("r3k2r/8/8/8/8/8/8/R4RK1"),
            "unexpected FEN: {after}"
        );
        // Queenside, both spellings: the h1 rook stays, the a1 rook lands on
        // d1 and the king on c1.
        let (after, _) = validate_and_apply(fen, &MoveInput::from_uci("e1c1").unwrap()).unwrap();
        assert!(
            after.starts_with("r3k2r/8/8/8/8/8/8/2KR3R"),
            "unexpected FEN: {after}"
        );
        let (after, _) = validate_and_apply(fen, &MoveInput::from_uci("e1a1").unwrap()).unwrap();
        assert!(
            after.starts_with("r3k2r/8/8/8/8/8/8/2KR3R"),
            "unexpected FEN: {after}"
        );
    }

    #[test]
    fn legal_moves_speak_standard_uci() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        let moves = legal_moves(fen).unwrap();
        assert!(
            moves.contains(&"e1g1".to_string()),
            "missing e1g1: {moves:?}"
        );
        assert!(
            moves.contains(&"e1c1".to_string()),
            "missing e1c1: {moves:?}"
        );
        assert!(
            !moves.contains(&"e1h1".to_string()),
            "engine spelling leaked"
        );
        assert!(
            !moves.contains(&"e1a1".to_string()),
            "engine spelling leaked"
        );
    }

    #[test]
    fn castling_is_rejected_when_the_path_is_attacked() {
        // A black bishop on c5 controls g1: white may not castle that way.
        let fen = "r3k2r/8/8/2b5/8/8/8/R3K2R w KQkq - 0 1";
        let mv = MoveInput::from_uci("e1g1").unwrap();
        assert!(matches!(
            validate_and_apply(fen, &mv),
            Err(MoveError::IllegalMove(_))
        ));
    }

    #[test]
    fn normalize_canonicalizes_castling_spellings() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        let mv = MoveInput::from_uci("e1h1").unwrap();
        assert_eq!(normalize(fen, &mv).unwrap().to_uci(), "e1g1");
        let mv = MoveInput::from_uci("e1a1").unwrap();
        assert_eq!(normalize(fen, &mv).unwrap().to_uci(), "e1c1");
        // Ordinary moves pass through unchanged.
        let mv = MoveInput::from_uci("e1e2").unwrap();
        assert_eq!(normalize(fen, &mv).unwrap().to_uci(), "e1e2");
        // Illegal moves are still rejected.
        assert!(normalize(fen, &MoveInput::from_uci("e1e5").unwrap()).is_err());
    }
}
