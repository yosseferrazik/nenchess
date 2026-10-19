import { useEffect, useState } from 'react';
import { useAuth } from '../lib/auth';
import { ApiError, http } from '../lib/http';
import type { ProfileDto } from '../lib/protocol';

export function ProfilePage() {
  const { session, logout } = useAuth();
  const [profile, setProfile] = useState<ProfileDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const p = await http.profile();
        if (!cancelled) setProfile(p);
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
      <button type="button" className="danger" onClick={logout}>
        Cerrar sesión
      </button>
    </section>
  );
}
