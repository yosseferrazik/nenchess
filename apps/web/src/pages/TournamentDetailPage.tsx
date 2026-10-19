import { useCallback, useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';

import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
import type { TournamentView } from '../lib/protocol';

const RESULT_LABEL: Record<string, string> = {
  white_win: '1 – 0',
  black_win: '0 – 1',
  draw: '½ – ½',
  bye: 'Bye',
};

export function TournamentDetailPage() {
  const { tournamentId } = useParams<{ tournamentId: string }>();
  const { session } = useAuth();
  const [view, setView] = useState<TournamentView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    if (!session || !tournamentId) return;
    setLoading(true);
    setError(null);
    try {
      const v = await http.getTournament(tournamentId);
      setView(v);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : 'no se pudo cargar el torneo');
    } finally {
      setLoading(false);
    }
  }, [session, tournamentId]);

  useEffect(() => {
    void load();
  }, [load]);

  // While the tournament is running, poll so standings and pairings stay live
  // as opponents finish their games.
  useEffect(() => {
    if (!view || view.tournament.status !== 'active') return;
    const id = window.setInterval(() => void load(), 4_000);
    return () => window.clearInterval(id);
  }, [view, load]);

  if (!session) return null;
  if (loading && !view) return <p className="muted">Cargando…</p>;
  if (error) return <div className="error-banner">{error}</div>;
  if (!view) return null;

  const { tournament, standings, rounds } = view;

  return (
    <section className="tournament-detail">
      <p>
        <Link to="/tournaments">← Torneos</Link>
      </p>
      <h2>{tournament.name}</h2>
      <p className="muted small">
        {tournament.time_control_initial_ms / 60_000}+{tournament.time_control_increment_ms / 1_000} ·{' '}
        {tournament.format === 'swiss' ? 'suizo' : 'todos contra todos'} ·{' '}
        {tournament.rated ? 'clasificatoria' : 'amistosa'} · ronda{' '}
        {tournament.current_round}/{tournament.total_rounds || '—'}
      </p>

      <div className="panel">
        <h3>Clasificación</h3>
        <table className="list">
          <thead>
            <tr>
              <th>#</th>
              <th>Jugador</th>
              <th>Puntos</th>
              <th>Partidas</th>
              <th>ELO inicial</th>
            </tr>
          </thead>
          <tbody>
            {standings.map((p, i) => (
              <tr key={p.user_id}>
                <td>{i + 1}</td>
                <td>{p.username}</td>
                <td>{p.points.toFixed(1)}</td>
                <td>{p.games_played}</td>
                <td>{p.rating_snapshot}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {rounds.map((round) => (
        <div className="panel" key={round.round_number}>
          <h3>Ronda {round.round_number}</h3>
          <table className="list">
            <thead>
              <tr>
                <th>Blancas</th>
                <th>Negras</th>
                <th>Resultado</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {round.pairings.map((pairing) => (
                <tr key={pairing.id}>
                  <td>{pairing.white_username ?? '—'}</td>
                  <td>{pairing.black_username ?? '—'}</td>
                  <td>
                    {pairing.result ? (RESULT_LABEL[pairing.result] ?? pairing.result) : '·'}
                  </td>
                  <td>
                    {pairing.game_id && (
                      <Link to={`/games/${pairing.game_id}`}>Ver partida</Link>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ))}
    </section>
  );
}
