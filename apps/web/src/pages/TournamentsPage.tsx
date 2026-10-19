import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';

import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
import type { CreateTournamentRequest, TournamentDto } from '../lib/protocol';

const STATUS_LABEL: Record<TournamentDto['status'], string> = {
  registration: 'Inscripción abierta',
  active: 'En curso',
  finished: 'Finalizado',
};

export function TournamentsPage() {
  const { session } = useAuth();
  const [tournaments, setTournaments] = useState<TournamentDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const [form, setForm] = useState<CreateTournamentRequest>({
    name: '',
    time_control_initial_ms: 300_000,
    time_control_increment_ms: 2_000,
    rated: true,
    max_participants: 8,
  });

  const load = useCallback(async () => {
    if (!session) return;
    setLoading(true);
    setError(null);
    try {
      const res = await http.listTournaments(session.token);
      setTournaments(res.tournaments);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : 'no se pudieron cargar los torneos');
    } finally {
      setLoading(false);
    }
  }, [session]);

  useEffect(() => {
    void load();
  }, [load]);

  if (!session) return null;

  const onCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try {
      await http.createTournament(session.token, form);
      setForm({ ...form, name: '' });
      await load();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'no se pudo crear el torneo');
    }
  };

  const act = async (id: string, fn: (token: string, id: string) => Promise<unknown>) => {
    setBusyId(id);
    setError(null);
    try {
      await fn(session.token, id);
      await load();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'la operación falló');
    } finally {
      setBusyId(null);
    }
  };

  return (
    <section className="tournaments">
      <h2>Torneos</h2>
      {error && <div className="error-banner">{error}</div>}

      <div className="panel">
        <h3>Crear torneo</h3>
        <form onSubmit={onCreate} className="tournament-form">
          <label>
            Nombre
            <input
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="Copa NenChess"
              required
            />
          </label>
          <label>
            Tiempo (min)
            <input
              type="number"
              min={1}
              max={60}
              value={form.time_control_initial_ms / 60_000}
              onChange={(e) =>
                setForm({ ...form, time_control_initial_ms: Number(e.target.value) * 60_000 })
              }
            />
          </label>
          <label>
            Incremento (s)
            <input
              type="number"
              min={0}
              max={60}
              value={form.time_control_increment_ms / 1_000}
              onChange={(e) =>
                setForm({ ...form, time_control_increment_ms: Number(e.target.value) * 1_000 })
              }
            />
          </label>
          <label>
            Participantes
            <input
              type="number"
              min={2}
              max={64}
              value={form.max_participants}
              onChange={(e) => setForm({ ...form, max_participants: Number(e.target.value) })}
            />
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={form.rated}
              onChange={(e) => setForm({ ...form, rated: e.target.checked })}
            />
            Clasificatoria (afecta al ELO)
          </label>
          <button type="submit">Crear</button>
        </form>
      </div>

      <div className="panel">
        <h3>Torneos</h3>
        {loading && <p className="muted small">Cargando...</p>}
        {!loading && tournaments.length === 0 && (
          <p className="muted small">No hay torneos todavía. ¡Crea el primero!</p>
        )}
        {tournaments.length > 0 && (
          <table className="list">
            <thead>
              <tr>
                <th>Nombre</th>
                <th>Estado</th>
                <th>Ronda</th>
                <th>Tiempo</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {tournaments.map((t) => (
                <tr key={t.id}>
                  <td>
                    <Link to={`/tournaments/${t.id}`}>{t.name}</Link>
                  </td>
                  <td>{STATUS_LABEL[t.status]}</td>
                  <td>
                    {t.current_round}/{t.total_rounds || '—'}
                  </td>
                  <td>
                    {t.time_control_initial_ms / 60_000}+{t.time_control_increment_ms / 1_000}
                  </td>
                  <td className="actions">
                    {t.status === 'registration' && (
                      <button
                        type="button"
                        disabled={busyId === t.id}
                        onClick={() => act(t.id, http.joinTournament)}
                      >
                        Unirme
                      </button>
                    )}
                    {t.status === 'registration' && t.created_by === session.user.id && (
                      <button
                        type="button"
                        disabled={busyId === t.id}
                        onClick={() => act(t.id, http.startTournament)}
                      >
                        Iniciar
                      </button>
                    )}
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
