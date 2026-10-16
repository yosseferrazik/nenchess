use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// A side to move or a player colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn opposite(self) -> Self {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Color::White => "white",
            Color::Black => "black",
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "white" => Ok(Color::White),
            "black" => Ok(Color::Black),
            other => Err(ParseColorError(other.to_string())),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid colour: {0}")]
pub struct ParseColorError(String);

/// Kind of piece, also used for promotions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PieceKind {
    Queen,
    Rook,
    Bishop,
    Knight,
}

impl PieceKind {
    /// Lower-case UCI promotion letter (`q`, `r`, `b`, `n`).
    pub fn uci_letter(self) -> char {
        match self {
            PieceKind::Queen => 'q',
            PieceKind::Rook => 'r',
            PieceKind::Bishop => 'b',
            PieceKind::Knight => 'n',
        }
    }

    pub fn from_uci_letter(c: char) -> Option<Self> {
        match c.to_ascii_lowercase() {
            'q' => Some(PieceKind::Queen),
            'r' => Some(PieceKind::Rook),
            'b' => Some(PieceKind::Bishop),
            'n' => Some(PieceKind::Knight),
            _ => None,
        }
    }
}

/// A board square, indexed `0..64` (`a1 = 0`, `h8 = 63`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Square(u8);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid square: {0}")]
pub struct ParseSquareError(String);

impl Square {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        (index < 64).then_some(Square(index as u8))
    }

    pub fn file(self) -> u8 {
        self.0 % 8
    }

    pub fn rank(self) -> u8 {
        self.0 / 8
    }
}

impl FromStr for Square {
    type Err = ParseSquareError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseSquareError(s.to_string());
        let bytes = s.as_bytes();
        if bytes.len() != 2 {
            return Err(err());
        }
        let file = bytes[0];
        let rank = bytes[1];
        if !(b'a'..=b'h').contains(&file) || !(b'1'..=b'8').contains(&rank) {
            return Err(err());
        }
        Ok(Square((rank - b'1') * 8 + (file - b'a')))
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = (b'a' + self.file()) as char;
        let rank = (b'1' + self.rank()) as char;
        write!(f, "{file}{rank}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_squares() {
        assert_eq!("e2".parse::<Square>().unwrap().to_string(), "e2");
        assert_eq!("a1".parse::<Square>().unwrap().index(), 0);
        assert_eq!("h8".parse::<Square>().unwrap().index(), 63);
        assert!("e9".parse::<Square>().is_err());
        assert!("i2".parse::<Square>().is_err());
        assert!("e".parse::<Square>().is_err());
    }

    #[test]
    fn colours_roundtrip() {
        assert_eq!("white".parse::<Color>().unwrap(), Color::White);
        assert_eq!(Color::Black.opposite(), Color::White);
        assert_eq!(Color::White.as_str(), "white");
    }
}
