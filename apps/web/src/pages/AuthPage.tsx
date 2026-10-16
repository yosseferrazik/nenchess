import { useState } from 'react';
import { Navigate, useNavigate } from 'react-router-dom';

import { useAuth } from '../lib/auth';
import { ApiError } from '../lib/http';

export function AuthPage() {
  const { session, login, register } = useAuth();
  const navigate = useNavigate();
  const [mode, setMode] = useState<'login' | 'register'>('login');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (session) return <Navigate to="/games" replace />;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === 'login') await login(username, password);
      else await register(username, password);
      navigate('/games');
    } catch (problem) {
      setError(problem instanceof ApiError ? problem.message : 'no se pudo completar la operación');
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="auth">
      <h2>{mode === 'login' ? 'Entrar' : 'Crear cuenta'}</h2>
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Usuario
          <input
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            autoComplete="username"
            minLength={3}
            maxLength={20}
            required
          />
        </label>
        <label>
          Contraseña
          <input
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
            minLength={8}
            required
          />
        </label>
        {error ? <div className="error-banner">{error}</div> : null}
        <button type="submit" disabled={busy}>
          {mode === 'login' ? 'Entrar' : 'Registrarme'}
        </button>
      </form>
      <button type="button" className="link" onClick={() => setMode(mode === 'login' ? 'register' : 'login')}>
        {mode === 'login' ? 'No tengo cuenta' : 'Ya tengo cuenta'}
      </button>
    </section>
  );
}
