//! Session context: the authenticated user, persisted in `localStorage` for
//! instant paint and re-validated against `/auth/me` on every page load.
//!
//! The session *token* never reaches JavaScript: it lives in an `HttpOnly`
//! cookie set by the server at login/registration. Only non-sensitive
//! display data (username, expiry) is stored client-side.

import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

import { http } from './http';
import type { UserDto } from './protocol';

export interface Session {
  user: UserDto;
  /** Display-only expiry from the login response; null when unknown. */
  expiresAtMs: number | null;
}

interface AuthContextValue {
  session: Session | null;
  /** True while a stored session is being revalidated. */
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
    const parsed = JSON.parse(raw) as Partial<Session>;
    if (!parsed.user) return null;
    return {
      user: parsed.user,
      expiresAtMs: typeof parsed.expiresAtMs === 'number' ? parsed.expiresAtMs : null,
    };
  } catch {
    return null;
  }
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session | null>(loadStored);
  const [checking, setChecking] = useState(() => loadStored() !== null);

  // Re-validate against the cookie: the stored session may have expired
  // server-side, and a fresh login (e.g. another tab) also lands here.
  useEffect(() => {
    let cancelled = false;
    http
      .me()
      .then((user) => {
        if (!cancelled) setSession({ user, expiresAtMs: loadStored()?.expiresAtMs ?? null });
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
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      session,
      checking,
      async login(username, password) {
        const response = await http.login({ username, password });
        const next: Session = {
          user: response.user,
          expiresAtMs: response.expires_at_ms,
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        setSession(next);
      },
      async register(username, password) {
        const response = await http.register({ username, password });
        const next: Session = {
          user: response.user,
          expiresAtMs: response.expires_at_ms,
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
        setSession(next);
      },
      logout() {
        // Clear locally at once; revoke server-side best-effort in the
        // background (a failed revoke only leaves the cookie alive until
        // it expires or the user logs in again).
        localStorage.removeItem(STORAGE_KEY);
        setSession(null);
        void http.logout().catch(() => undefined);
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
