//! REST client. Every call carries the bearer session token; every failure is
//! normalized into an `ApiError` carrying the server's `ErrorCode`.

import type {
  AuthResponse,
  CreateGameRequest,
  ErrorCode,
  GameView,
  GamesResponse,
  LoginRequest,
  RegisterRequest,
  UserDto,
} from './protocol';

export class ApiError extends Error {
  constructor(
    public readonly code: ErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

interface RequestOptions {
  method?: 'GET' | 'POST';
  token?: string;
  body?: unknown;
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) headers['content-type'] = 'application/json';
  if (options.token) headers['authorization'] = `Bearer ${options.token}`;

  const response = await fetch(`/api${path}`, {
    method: options.method ?? 'GET',
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  const text = await response.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = null;
    }
  }

  if (!response.ok) {
    const error = payload as { code?: ErrorCode; message?: string } | null;
    throw new ApiError(error?.code ?? 'internal', error?.message ?? response.statusText);
  }
  return payload as T;
}

export const http = {
  register: (body: RegisterRequest) =>
    request<AuthResponse>('/auth/register', { method: 'POST', body }),
  login: (body: LoginRequest) => request<AuthResponse>('/auth/login', { method: 'POST', body }),
  me: (token: string) => request<UserDto>('/auth/me', { token }),
  listGames: (token: string) => request<GamesResponse>('/games', { token }),
  createGame: (token: string, body: CreateGameRequest) =>
    request<GameView>('/games', { method: 'POST', token, body }),
  getGame: (token: string, gameId: string) => request<GameView>(`/games/${gameId}`, { token }),
  joinGame: (token: string, gameId: string) =>
    request<GameView>(`/games/${gameId}/join`, { method: 'POST', token, body: {} }),
};
