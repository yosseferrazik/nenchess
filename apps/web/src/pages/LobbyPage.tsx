import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';

import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
import { copyInviteLink } from '../lib/invite';
import type { GameView } from '../lib/protocol';

const TIME_CONTROLS = [
  { label: 'Bullet 1+0', initial: 60_000, increment: 0 },
  { label: 'Blitz 3+2', initial: 180_000, increment: 2_000 },
  { label: 'Blitz 5+0', initial: 300_000, increment: 0 },
  { label: 'Rápida 10+0', initial: 600_000, increment: 0 },
  { label: 'Rápida 15+10', initial: 900_000, increment: 10_000 },
];

export function LobbyPage() {
  const { session } = useAuth();
  const navigate = useNavigate();
  const [games, setGames] = useState<GameView[]>([]);
  const [control, setControl] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [searching, setSearching] = useState(false);
  const [queuedAt, setQueuedAt] = useState<number | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [rated, setRated] = useState(true);

  const refresh = useCallback(async () => {
    if (!session) return;
    try {
      const response = await http.listGames();
      setGames(response.games);
      setError(null);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo cargar el vestíbulo');
    }
  }, [session]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 5_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  // While queued, poll the server: it is the only party that decides when a
  // pairing happens, and the game id arrives exactly once.
  useEffect(() => {
    if (!searching || !session) return;
    let cancelled = false;

    const poll = async () => {
      try {
        const status = await http.matchmakingStatus();
        if (cancelled) return;
        if (status.game_id) {
          setSearching(false);
          navigate(`/games/${status.game_id}`);
          return;
        }
        if (status.in_queue) {
          const since = status.enqueued_at_ms ?? Date.now();
          setQueuedAt(since);
        }
      } catch (problem) {
        if (!cancelled) {
          setError(problem instanceof ApiError ? problem.message : 'se perdió la conexión');
        }
      }
    };

    void poll();
    const id = window.setInterval(() => void poll(), 1_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [searching, session, navigate]);

  const createGame = async () => {
    if (!session) return;
    setBusy(true);
    try {
      const game = await http.createGame({
        initial_time_ms: TIME_CONTROLS[control].initial,
        increment_ms: TIME_CONTROLS[control].increment,
        rated,
      });
      navigate(`/games/${game.id}`);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo crear la partida');
    } finally {
      setBusy(false);
    }
  };

  const findGame = async () => {
    if (!session) return;
    setBusy(true);
    setError(null);
    try {
      await http.enqueue({
        initial_time_ms: TIME_CONTROLS[control].initial,
        increment_ms: TIME_CONTROLS[control].increment,
        rated,
      });
      setQueuedAt(Date.now());
      setSearching(true);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo buscar rival');
    } finally {
      setBusy(false);
    }
  };

  const cancelSearch = async () => {
    if (!session) return;
    try {
      await http.dequeue();
    } catch {
      // Leaving the queue is best-effort; the next status poll is authoritative.
    }
    setSearching(false);
    setQueuedAt(null);
  };

  const joinGame = async (gameId: string) => {
    if (!session) return;
    setBusy(true);
    try {
      await http.joinGame(gameId);
      navigate(`/games/${gameId}`);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo unir a la partida');
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const copyLink = async (gameId: string) => {
    const ok = await copyInviteLink(gameId);
    setCopied(ok ? gameId : null);
    window.setTimeout(() => setCopied(null), 2_000);
  };

  const elapsed = queuedAt ? Math.max(0, Math.floor((Date.now() - queuedAt) / 1_000)) : 0;

  return (
    <section className="lobby">
      <h2>Vestíbulo</h2>
      {error ? <div className="error-banner">{error}</div> : null}

      <div className="panel">
        <h3>Buscar rival</h3>
        <label>
          Control de tiempo
          <select value={control} onChange={(event) => setControl(Number(event.target.value))}>
            {TIME_CONTROLS.map((option, index) => (
              <option key={option.label} value={index}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={rated}
            onChange={(event) => setRated(event.target.checked)}
          />
          Clasificatoria (afecta al ELO)
        </label>
        {searching ? (
          <>
            <p className="muted small">
              Buscando rival… {elapsed}s
              <br />
              El servidor te asignará una partida en cuanto haya alguien con el mismo control de
              tiempo. No hace falta recargar.
            </p>
            <button type="button" onClick={() => void cancelSearch()}>
              Cancelar búsqueda
            </button>
          </>
        ) : (
          <>
            <p className="muted small">
              El color lo decide el servidor. En cuanto pare, la partida se abre sola.
            </p>
            <button type="button" onClick={() => void findGame()} disabled={busy}>
              Buscar rival
            </button>
          </>
        )}
      </div>

      <div className="panel">
        <h3>Nueva partida</h3>
        <p className="muted small">
          Jugarás con blancas y podrás compartir el enlace de la partida. Los relojes se anclan
          cuando entra el segundo jugador.
        </p>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={rated}
            onChange={(event) => setRated(event.target.checked)}
          />
          Clasificatoria (afecta al ELO)
        </label>
        <button type="button" onClick={() => void createGame()} disabled={busy || searching}>
          Crear partida
        </button>
      </div>

      <div className="panel">
        <h3>Partidas abiertas</h3>
        {games.length === 0 ? (
          <p className="muted small">No hay partidas esperando rival.</p>
        ) : (
          <table className="list">
            <thead>
              <tr>
                <th>Blancas</th>
                <th>Ritmo</th>
                <th>Tipo</th>
                <th />
                <th />
              </tr>
            </thead>
            <tbody>
              {games.map((game) => (
                <tr key={game.id}>
                  <td>{game.white?.username ?? '—'}</td>
                  <td>
                    {Math.round(game.initial_time_ms / 1000)}s
                    {game.increment_ms > 0 ? ` + ${Math.round(game.increment_ms / 1000)}s` : ''}
                  </td>
                  <td>{game.rated ? 'Clasificatoria' : 'Amistosa'}</td>
                  <td>
                    <button
                      type="button"
                      onClick={() => void joinGame(game.id)}
                      disabled={busy || game.white?.user_id === session?.user.id}
                    >
                      {game.white?.user_id === session?.user.id ? 'tu partida' : 'Unirse'}
                    </button>
                  </td>
                  <td>
                    <button
                      type="button"
                      className="secondary"
                      onClick={() => void copyLink(game.id)}
                    >
                      {copied === game.id ? '¡copiado!' : 'Copiar enlace'}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}
