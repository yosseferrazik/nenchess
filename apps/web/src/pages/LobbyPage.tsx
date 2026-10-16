import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';

import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
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

  const refresh = useCallback(async () => {
    if (!session) return;
    try {
      const response = await http.listGames(session.token);
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

  const createGame = async () => {
    if (!session) return;
    setBusy(true);
    try {
      const game = await http.createGame(session.token, {
        initial_time_ms: TIME_CONTROLS[control].initial,
        increment_ms: TIME_CONTROLS[control].increment,
        rated: false,
      });
      navigate(`/games/${game.id}`);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo crear la partida');
    } finally {
      setBusy(false);
    }
  };

  const joinGame = async (gameId: string) => {
    if (!session) return;
    setBusy(true);
    try {
      await http.joinGame(session.token, gameId);
      navigate(`/games/${gameId}`);
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo unir a la partida');
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="lobby">
      <h2>Vestíbulo</h2>
      {error ? <div className="error-banner">{error}</div> : null}

      <div className="panel">
        <h3>Nueva partida</h3>
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
        <p className="muted small">
          Jugarás con blancas. Los relojes se anclan cuando entra el segundo jugador.
        </p>
        <button type="button" onClick={() => void createGame()} disabled={busy}>
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
                  <td>
                    <button
                      type="button"
                      onClick={() => void joinGame(game.id)}
                      disabled={busy || game.white?.user_id === session?.user.id}
                    >
                      {game.white?.user_id === session?.user.id ? 'tu partida' : 'Unirse'}
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
