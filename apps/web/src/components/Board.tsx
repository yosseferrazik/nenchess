//! Chessboard rendered straight from the authoritative FEN.
//!
//! The client deliberately contains **no chess rules**: clicking a piece then
//! a square expresses an *intent*, which the server validates. The board only
//! knows which pieces belong to the player to offer sane interaction.

import { useState } from 'react';
import type { PieceKindDto } from '../lib/protocol';

const GLYPHS: Record<string, string> = {
  K: '♔',
  Q: '♕',
  R: '♖',
  B: '♗',
  N: '♘',
  P: '♙',
  k: '♚',
  q: '♛',
  r: '♜',
  b: '♝',
  n: '♞',
  p: '♟',
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
        // FEN digits compress runs of *empty* squares; they must still be
        // emitted so the grid always holds exactly 64 clickable squares.
        for (let i = 0; i < Number(character); i += 1) {
          squares.push({ square: `${FILES[fileIndex]}${rank}`, piece: null });
          fileIndex += 1;
        }
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
  /** The player's color — only these pieces can be selected. */
  myColor: 'white' | 'black' | null;
  /** Player may express moves (they are seated in this game). */
  interactive: boolean;
  lastMove: { from: string; to: string } | null;
  onMove: (from: string, to: string, promotion: PieceKindDto | null) => void;
}

export function Board({ fen, orientation, myColor, interactive, lastMove, onMove }: BoardProps) {
  const [selected, setSelected] = useState<string | null>(null);
  const [promotion, setPromotion] = useState<{ from: string; to: string } | null>(null);

  const squares = parseBoard(fen);
  const bySquare = new Map(squares.map((entry) => [entry.square, entry.piece]));
  const ordered = orientation === 'white' ? squares : [...squares].reverse();

  const clear = () => {
    setSelected(null);
    setPromotion(null);
  };

  const isOwnPiece = (piece: string | null): boolean => {
    if (!piece || !myColor) return false;
    const isWhite = piece === piece.toUpperCase();
    return (myColor === 'white' && isWhite) || (myColor === 'black' && !isWhite);
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

    // Clicking one of our own pieces always (re)selects it; never treat it as
    // a move target. Clicking the already-selected square deselects.
    if (piece && isOwnPiece(piece)) {
      setSelected((current) => (current === square ? null : square));
      return;
    }

    if (selected && square !== selected) {
      if (wantsPromotion(selected, square)) {
        setPromotion({ from: selected, to: square });
        return;
      }
      onMove(selected, square, null);
      clear();
      return;
    }

    clear();
  };

  const choosePromotion = (piece: PieceKindDto) => {
    if (!promotion) return;
    onMove(promotion.from, promotion.to, piece);
    clear();
  };

  const promotionGlyph = (piece: PieceKindDto) => {
    const map = { queen: 'q', rook: 'r', bishop: 'b', knight: 'n' } as const;
    const glyph = map[piece];
    return myColor === 'black' ? glyph : glyph.toUpperCase();
  };

  // File labels go on the bottom rank from the viewer's perspective.
  const fileLabelRank = orientation === 'white' ? '1' : '8';

  return (
    <div className={`board-wrap ${interactive ? 'board-turn' : ''}`}>
      <div className="board" style={{ gridTemplateColumns: 'repeat(8, 1fr)', gridTemplateRows: 'repeat(8, 1fr)' }}>
        {ordered.map(({ square, piece }) => {
          const file = square[0];
          const rank = square[1];
          const dark = (FILES.indexOf(file) + Number(rank)) % 2 === 1;
          const classes = ['square', dark ? 'dark' : 'light'];
          if (square === selected) classes.push('selected');
          if (lastMove && (square === lastMove.from || square === lastMove.to)) {
            classes.push('last-move');
          }
          const own = piece && isOwnPiece(piece);
          if (own && interactive) classes.push('own');
          return (
            <button
              type="button"
              key={square}
              className={classes.join(' ')}
              onClick={() => handleClick(square)}
              disabled={!interactive}
              aria-label={square}
            >
              {piece ? (
                <span className={piece === piece.toUpperCase() ? 'white-piece' : 'black-piece'}>
                  {GLYPHS[piece]}
                </span>
              ) : null}
              {square[0] === 'a' ? <span className="coord rank">{rank}</span> : null}
              {square[1] === fileLabelRank ? <span className="coord file">{file}</span> : null}
            </button>
          );
        })}
      </div>

      {promotion ? (
        <div className="promotion">
          <span className="promotion-label">Promotion</span>
          <div className="promotion-pieces">
            {(['queen', 'rook', 'bishop', 'knight'] as PieceKindDto[]).map((piece) => (
              <button key={piece} type="button" className="promotion-btn" onClick={() => choosePromotion(piece)}>
                <span className={myColor === 'black' ? 'black-piece' : 'white-piece'}>
                  {GLYPHS[promotionGlyph(piece)]}
                </span>
              </button>
            ))}
          </div>
          <button type="button" className="promotion-cancel" onClick={clear}>
            cancel
          </button>
        </div>
      ) : null}
    </div>
  );
}
