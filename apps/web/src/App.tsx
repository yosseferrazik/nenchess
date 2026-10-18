import type { ReactElement } from 'react';
import { Link, Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom';

import { useAuth } from './lib/auth';
import { AuthPage } from './pages/AuthPage';
import { GamePage } from './pages/GamePage';
import { LobbyPage } from './pages/LobbyPage';
import { ProfilePage } from './pages/ProfilePage';
import { TournamentsPage } from './pages/TournamentsPage';

function Protected({ children }: { children: ReactElement }) {
  const { session, checking } = useAuth();
  const location = useLocation();
  if (checking) return <p className="muted">Verificando sesión…</p>;
  // Remember where the visitor was heading so an invite link survives the
  // login/registration round-trip.
  return session ? children : <Navigate to="/auth" replace state={{ from: location.pathname }} />;
}

function GameRoute() {
  const { gameId } = useParams<{ gameId: string }>();
  if (!gameId) return <Navigate to="/games" replace />;
  return <GamePage gameId={gameId} />;
}

export function App() {
  const { session, logout } = useAuth();

  return (
    <div className="app">
      <header className="topbar">
        <Link to="/games" className="brand">
          ♞ NenChess
        </Link>
        <nav>
          {session ? (
            <>
              <Link to="/games">Vestíbulo</Link>
              <Link to="/tournaments">Torneos</Link>
              <Link to="/profile">Perfil</Link>
              <span className="muted small">{session.user.username}</span>
              <button type="button" className="link" onClick={logout}>
                salir
              </button>
            </>
          ) : null}
        </nav>
      </header>

      <main>
        <Routes>
          <Route path="/" element={<Navigate to={session ? '/games' : '/auth'} replace />} />
          <Route path="/auth" element={<AuthPage />} />
          <Route
            path="/games"
            element={
              <Protected>
                <LobbyPage />
              </Protected>
            }
          />
          <Route
            path="/games/:gameId"
            element={
              <Protected>
                <GameRoute />
              </Protected>
            }
          />
          <Route
            path="/tournaments"
            element={
              <Protected>
                <TournamentsPage />
              </Protected>
            }
          />
          <Route
            path="/profile"
            element={
              <Protected>
                <ProfilePage />
              </Protected>
            }
          />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>

      <footer className="muted small">
        Servidor autoritativo · protocolo v1 · los tipos del contrato se generan desde Rust
      </footer>
    </div>
  );
}
