import { useAuth } from '../lib/auth';

export function ProfilePage() {
  const { session, logout } = useAuth();
  if (!session) return null;

  return (
    <section className="profile">
      <h2>Perfil</h2>
      <div className="panel">
        <dl>
          <dt>Usuario</dt>
          <dd>{session.user.username}</dd>
          <dt>Identificador</dt>
          <dd>
            <code>{session.user.id}</code>
          </dd>
          <dt>Sesión expira</dt>
          <dd>{new Date(session.expiresAtMs).toLocaleString()}</dd>
        </dl>
      </div>
      <div className="panel">
        <h3>Estadísticas</h3>
        <p className="muted small">
          El ELO y el historial de resultados llegan en la etapa 05 (módulo <code>ratings</code>), calculados
          siempre por el servidor a partir de las partidas terminadas.
        </p>
      </div>
      <button type="button" className="danger" onClick={logout}>
        Cerrar sesión
      </button>
    </section>
  );
}
