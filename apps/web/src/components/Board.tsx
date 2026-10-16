//! Chessboard rendered straight from the authoritative FEN.
//!
//! The client deliberately contains **no chess rules**: clicking a piece then
//! a square expresses an *intent*, which the server validates. The board only
//! knows which pieces belong to the player (colour case) to offer sane
//! interaction.

import { useState } from 'react';
import type { PieceKindDto } from '../lib/protocol';

const GLYPHS: Record<string, string> = {
  K: '\u2654',
  Q: '\u2655',
  R: '\u2656',
  B: '\u2657',
  N: '\u2658',
  P: '\u2659',
  k: '\u265A',
  q: '\u265B',
  r: '\u265C',
  b: '\u265D',
  n: '\u265E',
  p: '\u265F',
};

const FILES = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];

interface PlacedSquare {
  square: string;
  piece: string | null;
}

function parseBoard(fen: string): PlacedSquare[] {
  const placement = fen.split(' ')[0] ?? '';
  const rows = placement.split('/');
  const squares: PlacedSquare[] = [];
  rows.forEach((row, rowIndex) => {
    const rank = 8 - rowIndex;
    let fileIndex = 0;
    for (const character of row) {
      if (character >= '1' && character <= '8') {
        fileIndex += Number(character);
        continue;
      }
      squares.push({ square: `${FILES[fileIndex]}${rank}`, piece: character });
      fileIndex += 1;
    }
  });
  return squares;
}

export interface BoardProps {
  fen: string;
  orientation: 'white' | 'black';
  /** Player may express moves (they are seated in this game). */
  interactive: boolean;
  lastMove: { from: string; to: string } | null;
  onMove: (from: string, to: string, promotion: PieceKindDto | null) => void;
}

export function Board({ fen, orientation, interactive, lastMove, onMove }: BoardProps) {
  const [selected, setSelected] = useState<string | null>(null);
  const [promotion, setPromotion] = useState<{ from: string; to: string } | null>(null);

  const squares = parseBoard(fen);
  const bySquare = new Map(squares.map((entry) => [entry.square, entry.piece]));
  const ordered = orientation === 'white' ? squares : [...squares].reverse();

  const clear = () => {
    setSelected(null);
    setPromotion(null);
  };

  const wantsPromotion = (from: string, to: string) => {
    const piece = bySquare.get(from);
    if (!piece) return false;
    if (piece !== 'p' && piece !== 'P') return false;
    return (piece === 'P' && to.endsWith('8')) || (piece === 'p' && to.endsWith('1'));
  };

  const handleClick = (square: string) => {
    if (!interactive) return;
    const piece = bySquare.get(square);

    if (selected && square !== selected) {
      if (wantsPromotion(selected, square)) {
        setPromotion({ from: selected, to: square });
        return;
      }
      onMove(selected, square, null);
      clear();
      return;
    }

    // Select one of your own pieces (white = uppercase).
    if (piece && piece === piece.toUpperCase()) {
      setSelected(square);
      return;
    }
    if (piece && piece === piece.toLowerCase()) {
      setSelected(square);
      return;
    }
    clear();
  };

  const choosePromotion = (piece: PieceKindDto) => {
    if (!promotion) return;
    onMove(promotion.from, promotion.to, piece);
    clear();
  };

  return (
    <div className="board-wrap">
      <div className="board" style={{ gridTemplateColumns: 'repeat(8, 1fr)' }}>
        {ordered.map(({ square, piece }) => {
          const file = square[0];
          const rank = square[1];
          const dark = (FILES.indexOf(file) + Number(rank)) % 2 === 1;
          const classes = ['square', dark ? 'dark' : 'light'];
          if (square === selected) classes.push('selected');
          if (lastMove && (square === lastMove.from || square === lastMove.to)) {
            classes.push('last-move');
          }
          return (
            <button
              type="button"
              key={square}
              className={classes.join(' ')}
              onClick={() => handleClick(square)}
              disabled={!interactive}
              aria-label={square}
            >
              {piece ? <span className={piece === piece.toUpperCase() ? 'white-piece' : 'black-piece'}>{GLYPHS[piece]}</span> : null}
              {square[0] === 'a' ? <span className="coord rank">{rank}</span> : null}
              {square[1] === '8' ? <span className="coord file">{file}</span> : null}
            </button>
          );
        })}
      </div>

      {promotion ? (
        <div className="promotion">
          <span>Promotion:</span>
          {(['queen', 'rook', 'bishop', 'knight'] as PieceKindDto[]).map((piece) => (
            <button key={piece} type="button" onClick={() => choosePromotion(piece)}>
              {piece}
            </button>
          ))}
          <button type="button" onClick={clear}>
            cancel
          </button>
        </div>
      ) : null}
    </div>
  );
}
