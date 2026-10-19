import { useState } from 'react';

import { useGameSocket } from '../lib/ws';
import { Board } from '../components/Board';
import { Clock } from '../components/Clock';
import { useAuth } from '../lib/auth';
import { copyInviteLink } from '../lib/invite';
import type { GameView, ResultReasonDto } from '../lib/protocol';

/** Human-readable reason, shown to spectators and to the players. */
const REASON_LABELS: Record<ResultReasonDto, string> = {
  checkmate: 'jaque mate',
  stalemate: 'ahogado',
  resignation: 'abandono',
  flag: 'caída de bandera (tiempo)',
  fifty_move: 'regla de 50 movimientos',
  threefold_repetition: 'triple repetición',
  insufficient_material: 'material insuficiente',
  abandonment: 'abandono por ausencia',
  agreement: 'acuerdo entre los jugadores',
};

export function GamePage({ gameId }: { gameId: string }) {
  const { session } = useAuth();
  const { state, move, resign, offerDraw, acceptDraw, declineDraw, clearError } =
    useGameSocket(gameId);
  const { game } = state;
  const [linkState, setLinkState] = useState<'idle' | 'copied' | 'failed'>('idle');

  const myColor = colorOf(game, session?.user.id ?? null);
  const lastMove = lastMoveOf(game);
  const interactive =
    game?.status === 'active' && myColor !== null && game.turn === myColor && !state.pending;

  const copyLink = async () => {
    const ok = await copyInviteLink(gameId);
    setLinkState(ok ? 'copied' : 'failed');
    window.setTimeout(() => setLinkState('idle'), 2_000);
  };

  return (
    <section className="game">
      <header className="game-header">
        <h2>Partida</h2>
        <span className={`badge ${state.status}`}>
          {state.status === 'open' ? 'en línea' : state.status === 'connecting' ? 'conectando…' : 'desconectado'}
        </span>
      </header>

      {state.lastError ? (
        <div className="error-banner" onClick={clearError} role="alert">
          {state.lastError} <small>(clic para descartar)</small>
        </div>
      ) : null}

      {!game ? (
        <p className="muted">Cargando partida…</p>
      ) : (
        <div className="game-layout">
          <div className="board-column">
            <PlayerRow game={game} side="black" fallback="(esperando rival)" />
            <Board
              fen={game.fen}
              orientation={myColor === 'black' ? 'black' : 'white'}
              myColor={myColor}
              interactive={interactive}
              lastMove={lastMove}
              onMove={(from, to, promotion) => {
                // Server errors arrive as `error` frames and are surfaced via
                // `state.lastError`; a transport failure just means the socket
                // will reconnect and resubscribe.
                void move(from, to, promotion).catch(() => undefined);
              }}
            />
            <PlayerRow game={game} side="white" fallback={myColor === null ? '(asiento vacío)' : ''} />
          </div>

          <aside className="side-panel">
            <StatusPanel game={game} myColor={myColor} />
            <DrawPanel
              game={game}
              myColor={myColor}
              pending={state.pending}
              offerDraw={() => void offerDraw().catch(() => undefined)}
              acceptDraw={() => void acceptDraw().catch(() => undefined)}
              declineDraw={() => void declineDraw().catch(() => undefined)}
            />
            <button type="button" className="secondary" onClick={() => void copyLink()}>
              {linkState === 'copied'
                ? '¡enlace copiado!'
                : linkState === 'failed'
                  ? 'no se pudo copiar'
                  : 'Copiar enlace de invitación'}
            </button>
            {myColor !== null && game.status === 'active' ? (
              <button type="button" className="danger" onClick={() => { if (confirm('¿Abandonar la partida?')) void resign(); }}>
                Abandonar
              </button>
            ) : null}
            <h3>Jugadas</h3>
            <MoveList game={game} />
            {game.id ? (
              <p className="muted small">
                id de partida: <code>{game.id}</code>
              </p>
            ) : null}
          </aside>
        </div>
      )}
    </section>
  );
}

function colorOf(game: GameView | null, userId: string | null): 'white' | 'black' | null {
  if (!game || !userId) return null;
  if (game.white?.user_id === userId) return 'white';
  if (game.black?.user_id === userId) return 'black';
  return null;
}

function lastMoveOf(game: GameView | null) {
  if (!game || game.moves.length === 0) return null;
  const last = game.moves[game.moves.length - 1];
  return { from: last.from, to: last.to };
}

function PlayerRow({ game, side, fallback }: { game: GameView; side: 'white' | 'black'; fallback: string }) {
  const player = side === 'white' ? game.white : game.black;
  const clock = game.clock;
  return (
    <div className="player-row">
      <span className="player-name">
        {player?.username ?? fallback}
        <small className="muted"> {side === 'white' ? '· blancas' : '· negras'}</small>
      </span>
      <Clock clock={clock} side={side} label={side === 'white' ? 'blancas' : 'negras'} />
    </div>
  );
}

function StatusPanel({ game, myColor }: { game: GameView; myColor: 'white' | 'black' | null }) {
  if (game.status === 'waiting') {
    return (
      <div className="status-panel">
        <strong>Esperando rival</strong>
        <p className="muted small">
          Comparte el enlace de invitación o el id de la partida para que se unan.
        </p>
      </div>
    );
  }
  if (game.status === 'finished') {
    const result = game.result;
    const winner =
      result?.winner === 'white' ? game.white?.username : result?.winner === 'black' ? game.black?.username : null;
    return (
      <div className="status-panel">
        <strong>{winner ? `Ganó ${winner}` : 'Tablas'}</strong>
        <p className="muted small">
          Motivo: {result ? (REASON_LABELS[result.reason] ?? result.reason) : 'desconocido'}
        </p>
      </div>
    );
  }
  return (
    <div className="status-panel">
      <strong>
        {myColor === null
          ? 'Observando'
          : game.turn === myColor
            ? 'Tu turno'
            : 'Turno del rival'}
      </strong>
      <p className="muted small">
        versión {game.version} · {game.move_count} jugadas · {Math.round(game.initial_time_ms / 1000)}s
        {game.increment_ms > 0 ? ` + ${Math.round(game.increment_ms / 1000)}s` : ''}
      </p>
    </div>
  );
}

/** Offer, accept, decline or withdraw a draw — only for seated players. */
function DrawPanel({
  game,
  myColor,
  pending,
  offerDraw,
  acceptDraw,
  declineDraw,
}: {
  game: GameView;
  myColor: 'white' | 'black' | null;
  pending: boolean;
  offerDraw: () => void;
  acceptDraw: () => void;
  declineDraw: () => void;
}) {
  if (myColor === null || game.status !== 'active') return null;

  if (game.draw_offered_by === myColor) {
    return (
      <div className="status-panel">
        <strong>Oferta de tablas enviada</strong>
        <p className="muted small">A la respuesta del rival… o retírala si te arrepientes.</p>
        <button type="button" className="secondary" disabled={pending} onClick={declineDraw}>
          Retirar la oferta
        </button>
      </div>
    );
  }
  if (game.draw_offered_by !== null) {
    return (
      <div className="status-panel">
        <strong>¡El rival propone tablas!</strong>
        <button type="button" disabled={pending} onClick={acceptDraw}>
          Aceptar tablas
        </button>
        <button type="button" className="secondary" disabled={pending} onClick={declineDraw}>
          Rechazar
        </button>
      </div>
    );
  }
  return (
    <button type="button" className="secondary" disabled={pending} onClick={offerDraw}>
      Proponer tablas
    </button>
  );
}

function MoveList({ game }: { game: GameView }) {
  if (game.moves.length === 0) return <p className="muted small">Sin jugadas todavía.</p>;

  const rows: { number: number; white?: string; black?: string }[] = [];
  game.moves.forEach((move) => {
    const number = Math.ceil(move.move_number / 2);
    let row = rows.find((entry) => entry.number === number);
    if (!row) {
      row = { number };
      rows.push(row);
    }
    if (move.move_number % 2 === 1) row.white = move.uci;
    else row.black = move.uci;
  });

  return (
    <ol className="moves">
      {rows.map((row) => (
        <li key={row.number}>
          <span className="move-number">{row.number}.</span>
          <span className="uci">{row.white ?? '…'}</span>
          <span className="uci">{row.black ?? ''}</span>
        </li>
      ))}
    </ol>
  );
}
