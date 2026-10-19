//! PGN/SAN notation, generated from the authoritative move records.
//!
//! The engine stays behind the kernel: callers only ever see `game-core`
//! types. SAN is *derived*, never parsed — it exists for human-readable
//! exports (PGN), never to accept input.

use cozy_chess::{Board, Move as EngineMove, Piece, Square as EngineSquare};

use crate::rules::{to_engine_move, MoveError, MoveInput};

/// The SAN of a legal move in the position `fen`: `e4`, `Nbd7`, `exd5`,
/// `e8=Q+`, `O-O`, `Qh4#`. The move must be legal there; anything else is
/// rejected exactly like the validation pipeline does. Castling may be given
/// in either accepted spelling; it always renders as `O-O`/`O-O-O`.
pub fn san(fen: &str, mv: &MoveInput) -> Result<String, MoveError> {
    let board = Board::from_fen(fen, false).map_err(|_| MoveError::InvalidFen(fen.to_string()))?;
    let uci = mv.to_uci();
    let engine_move = to_engine_move(&board, mv)?;
    if !board.is_legal(engine_move) {
        return Err(MoveError::IllegalMove(uci));
    }

    let from = engine_move.from;
    let to = engine_move.to;
    let piece = board
        .piece_on(from)
        .expect("a legal move starts on an occupied square");
    // En-passant captures land on an empty square; a pawn changing file while
    // not capturing is impossible (it would not be legal).
    let capture =
        board.color_on(to).is_some() || (piece == Piece::Pawn && from.file() != to.file());

    let mut san = if piece == Piece::King && from.file() != to.file() {
        // Castling is the only king move that changes files.
        if to.file() < from.file() {
            "O-O-O".to_string()
        } else {
            "O-O".to_string()
        }
    } else if piece == Piece::Pawn {
        if capture {
            format!("{}x{}", file_letter(from), square_name(to))
        } else {
            square_name(to)
        }
    } else {
        let mut text = piece_letter(piece).to_string();
        text.push_str(&disambiguator(&board, engine_move, piece));
        if capture {
            text.push('x');
        }
        text.push_str(&square_name(to));
        text
    };

    if let Some(promotion) = engine_move.promotion {
        san.push('=');
        san.push(piece_letter(promotion));
    }

    // Check and mate suffixes come from the position after the move.
    let mut after = board;
    after
        .try_play(engine_move)
        .map_err(|_| MoveError::IllegalMove(uci))?;
    if !after.checkers().is_empty() {
        let mut replies = 0usize;
        after.generate_moves(|piece_moves| {
            if !piece_moves.is_empty() {
                replies += 1;
            }
            false
        });
        san.push(if replies == 0 { '#' } else { '+' });
    }
    Ok(san)
}

/// File/rank/both disambiguation when several identical pieces can reach the
/// same square (SAN §4.3: the cheapest unique qualifier wins).
fn disambiguator(board: &Board, mv: EngineMove, piece: Piece) -> String {
    let mut competitors: Vec<EngineMove> = Vec::new();
    board.generate_moves(|piece_moves| {
        for candidate in piece_moves {
            if candidate.to == mv.to
                && candidate.from != mv.from
                && board.piece_on(candidate.from) == Some(piece)
            {
                competitors.push(candidate);
            }
        }
        false
    });
    if competitors.is_empty() {
        return String::new();
    }
    let same_file = competitors
        .iter()
        .any(|other| other.from.file() == mv.from.file());
    let same_rank = competitors
        .iter()
        .any(|other| other.from.rank() == mv.from.rank());
    if !same_file {
        file_letter(mv.from).to_string()
    } else if !same_rank {
        rank_letter(mv.from).to_string()
    } else {
        format!("{}{}", file_letter(mv.from), rank_letter(mv.from))
    }
}

fn file_letter(square: EngineSquare) -> char {
    (b'a' + square.file() as u8) as char
}

fn rank_letter(square: EngineSquare) -> char {
    (b'1' + square.rank() as u8) as char
}

fn square_name(square: EngineSquare) -> String {
    format!("{}{}", file_letter(square), rank_letter(square))
}

fn piece_letter(piece: Piece) -> char {
    match piece {
        Piece::Pawn => 'P',
        Piece::Knight => 'N',
        Piece::Bishop => 'B',
        Piece::Rook => 'R',
        Piece::Queen => 'Q',
        Piece::King => 'K',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::validate_and_apply;

    fn play(fen: &str, ucis: &[&str]) -> (String, Vec<String>) {
        let mut current = fen.to_string();
        let mut sans = Vec::new();
        for uci in ucis {
            let mv = MoveInput::from_uci(uci).unwrap();
            sans.push(san(&current, &mv).unwrap());
            current = validate_and_apply(&current, &mv).unwrap().0;
        }
        (current, sans)
    }

    #[test]
    fn renders_simple_and_checkmate_moves() {
        let (_, sans) = play(
            crate::rules::starting_fen(),
            &["f2f3", "e7e5", "g2g4", "d8h4"],
        );
        assert_eq!(sans, vec!["f3", "e5", "g4", "Qh4#"]);
    }

    #[test]
    fn renders_developing_and_castling_moves() {
        // Black avoids ...Bc5, which would control g1 and make castling
        // illegal — a good reminder that the SAN generator only ever renders
        // moves that are legal in the position.
        let (_, sans) = play(
            crate::rules::starting_fen(),
            &["e2e4", "e7e5", "g1f3", "g8f6", "f1c4", "b8c6", "e1g1"],
        );
        assert_eq!(sans, vec!["e4", "e5", "Nf3", "Nf6", "Bc4", "Nc6", "O-O"]);
    }

    #[test]
    fn renders_queenside_castling() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        let mv = MoveInput::from_uci("e1c1").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "O-O-O");
        let mv = MoveInput::from_uci("e1g1").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "O-O");
    }

    #[test]
    fn disambiguates_by_file_and_rank() {
        // Two knights share a rank and can both reach d2: the file qualifies.
        let fen = "4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1";
        let mv = MoveInput::from_uci("b1d2").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "Nbd2");
        let mv = MoveInput::from_uci("f1d2").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "Nfd2");
        // Two knights share a file and can both reach d2: the rank qualifies.
        let fen = "4k3/8/8/8/8/1N6/8/1N2K3 w - - 0 1";
        let mv = MoveInput::from_uci("b1d2").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "N1d2");
        let mv = MoveInput::from_uci("b3d2").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "N3d2");
    }

    #[test]
    fn renders_captures_promotions_and_en_passant() {
        let fen = "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1";
        let mv = MoveInput::from_uci("e5d6").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "exd6");

        let fen = "6k1/4P3/8/8/8/8/8/4K3 w - - 0 1";
        let mv = MoveInput::from_uci("e7e8q").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "e8=Q+");

        // Back-rank mate: the pawns on f7/g7/h7 cover every escape.
        let fen = "6k1/5ppp/8/8/8/8/8/4Q1K1 w - - 0 1";
        let mv = MoveInput::from_uci("e1e8").unwrap();
        assert_eq!(san(fen, &mv).unwrap(), "Qe8#");
    }

    #[test]
    fn rejects_illegal_moves() {
        let mv = MoveInput::from_uci("e2e5").unwrap();
        assert!(matches!(
            san(crate::rules::starting_fen(), &mv),
            Err(MoveError::IllegalMove(_))
        ));
    }
}
