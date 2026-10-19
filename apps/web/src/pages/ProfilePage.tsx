import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
import type { GameHistoryResponse, GameOutcomeDto, ProfileDto } from '../lib/protocol';

const OUTCOME_LABELS: Record<GameOutcomeDto, string> = {
  win: 'Victoria',
  draw: 'Tablas',
  loss: 'Derrota',
};

function outcomeLabel(outcome: GameOutcomeDto | null): string {
  return outcome === null ? 'En curso' : OUTCOME_LABELS[outcome];
}

function timeControlLabel(initialTimeMs: number, incrementMs: number): string {
  const initial = Math.round(initialTimeMs / 1000);
  const increment = Math.round(incrementMs / 1000);
  return increment > 0 ? `${initial}s + ${increment}s` : `${initial}s`;
}

export function ProfilePage() {
  const { session, logout } = useAuth();
  const navigate = useNavigate();
  const [profile, setProfile] = useState<ProfileDto | null>(null);
  const [history, setHistory] = useState<GameHistoryResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const [p, h] = await Promise.all([http.profile(), http.gameHistory()]);
        if (!cancelled) {
          setProfile(p);
          setHistory(h);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof ApiError ? e.message : 'no se pudo cargar el perfil');
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [session]);

  if (!session) return null;

  const user = profile?.user ?? session.user;

  return (
    <section className="profile">
      <h2>Perfil</h2>
      <div className="panel">
        <dl>
          <dt>Usuario</dt>
          <dd>{user.username}</dd>
          <dt>Identificador</dt>
          <dd>
            <code>{user.id}</code>
          </dd>
          <dt>Sesión expira</dt>
          <dd>
            {session.expiresAtMs ? new Date(session.expiresAtMs).toLocaleString() : '—'}
          </dd>
        </dl>
      </div>
      <div className="panel">
        <h3>Clasificación</h3>
        {loading && <p className="muted small">Cargando...</p>}
        {error && <div className="error-banner">{error}</div>}
        {!loading && !error && profile && (
          <table className="list">
            <thead>
              <tr>
                <th>Categoría</th>
                <th>ELO</th>
                <th>Partidas</th>
              </tr>
            </thead>
            <tbody>
              {profile.ratings.map((r) => (
                <tr key={r.category}>
                  <td>{r.category}</td>
                  <td>{r.rating}</td>
                  <td>{r.games_played}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      <div className="panel">
        <h3>Historial de partidas</h3>
        {!history || history.games.length === 0 ? (
          <p className="muted small">
            {loading ? 'Cargando...' : 'Todavía no hay partidas que mostrar.'}
          </p>
        ) : (
          <table className="list">
            <thead>
              <tr>
                <th>Resultado</th>
                <th>Rival</th>
                <th>Color</th>
                <th>Tiempo</th>
                <th>Fecha</th>
                <th>PGN</th>
              </tr>
            </thead>
            <tbody>
              {history.games.map((game) => (
                <tr key={game.game_id}>
                  <td>{outcomeLabel(game.outcome)}</td>
                  <td>
                    <button
                      type="button"
                      className="secondary"
                      onClick={() => navigate(`/games/${game.game_id}`)}
                    >
                      {game.opponent?.username ?? '—'}
                    </button>
                  </td>
                  <td>{game.color === 'white' ? 'Blancas' : 'Negras'}</td>
                  <td>{timeControlLabel(game.initial_time_ms, game.increment_ms)}</td>
                  <td>{new Date(game.started_at_ms).toLocaleDateString()}</td>
                  <td>
                    <a href={http.pgnUrl(game.game_id)} download>
                      PGN
                    </a>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      <button type="button" className="danger" onClick={logout}>
        Cerrar sesión
      </button>
    </section>
  );
}
