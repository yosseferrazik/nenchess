//! Session context: the bearer token plus the authenticated user, persisted
//! in `localStorage` and re-validated once per page load.

import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

import { http } from './http';
import type { UserDto } from './protocol';

export interface Session {
  token: string;
  user: UserDto;
  expiresAtMs: number;
}

interface AuthContextValue {
  session: Session | null;
  /** True while the stored session is being revalidated. */
  checking: boolean;
  login: (username: string, password: string) => Promise<void>;
  register: (username: string, password: string) => Promise<void>;
  logout: () => void;
}

const STORAGE_KEY = 'nenchess.session';

const AuthContext = createContext<AuthContextValue | null>(null);

function loadStored(): Session | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Session;
    if (!parsed.token || !parsed.user) return null;
    return parsed;
  } catch {
    return null;
  }
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(loadStored);
  const [checking, setChecking] = useState(() => loadStored() !== null);

  // Re-validate the stored token once: it may have expired server-side.
  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    http
      .me(session.token)
      .then((user) => {
        if (!cancelled) setSession({ ...session, user });
      })
      .catch(() => {
        if (!cancelled) {
          localStorage.removeItem(STORAGE_KEY);
          setSession(null);
        }
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      session,
      checking,
      async login(username, password) {
        const response = await http.login({ username, password });
        const next: Session = {
          token: response.token,
          user: response.user,
          expiresAtMs: response.expires_at_ms,
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        setSession(next);
      },
      async register(username, password) {
        const response = await http.register({ username, password });
        const next: Session = {
          token: response.token,
          user: response.user,
          expiresAtMs: response.expires_at_ms,
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        setSession(next);
      },
      logout() {
        localStorage.removeItem(STORAGE_KEY);
        setSession(null);
      },
    }),
    [session, checking],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

// eslint-disable-next-line react-refresh/only-export-components
export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) throw new Error('useAuth must be used inside AuthProvider');
  return context;
}
