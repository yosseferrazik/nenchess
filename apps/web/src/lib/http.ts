//! REST client. The session token lives in an `HttpOnly` cookie set at
//! login/registration, so requests only need `credentials: 'same-origin'`;
//! no token ever touches JavaScript. Every failure is normalized into an
//! `ApiError` carrying the server's `ErrorCode`.

import type {
  AuthResponse,
  CreateGameRequest,
  CreateTournamentRequest,
  ErrorCode,
  GameView,
  GameHistoryResponse,
  GamesResponse,
  LoginRequest,
  MatchmakingRequest,
  MatchmakingStatusDto,
  ProfileDto,
  RegisterRequest,
  TournamentDto,
  TournamentView,
  TournamentsResponse,
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
  method?: 'GET' | 'POST' | 'DELETE';
  body?: unknown;
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) headers['content-type'] = 'application/json';

  const response = await fetch(`/api${path}`, {
    method: options.method ?? 'GET',
    headers,
    credentials: 'same-origin',
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
  logout: () => request<null>('/auth/logout', { method: 'POST' }),
  me: () => request<UserDto>('/auth/me'),
  listGames: () => request<GamesResponse>('/games'),
  createGame: (body: CreateGameRequest) =>
    request<GameView>('/games', { method: 'POST', body }),
  getGame: (gameId: string) => request<GameView>(`/games/${gameId}`),
  joinGame: (gameId: string) =>
    request<GameView>(`/games/${gameId}/join`, { method: 'POST', body: {} }),
  enqueue: (body: MatchmakingRequest) =>
    request<MatchmakingStatusDto>('/matchmaking', { method: 'POST', body }),
  matchmakingStatus: () => request<MatchmakingStatusDto>('/matchmaking'),
  dequeue: () => request<MatchmakingStatusDto>('/matchmaking', { method: 'DELETE' }),
  profile: () => request<ProfileDto>('/profile'),
  gameHistory: () => request<GameHistoryResponse>('/me/games'),
  // Same-origin link: the session cookie authenticates it, no token in JS.
  pgnUrl: (gameId: string) => `/api/games/${gameId}/pgn`,
  listTournaments: () => request<TournamentsResponse>('/tournaments'),
  createTournament: (body: CreateTournamentRequest) =>
    request<TournamentDto>('/tournaments', { method: 'POST', body }),
  getTournament: (id: string) => request<TournamentView>(`/tournaments/${id}`),
  joinTournament: (id: string) =>
    request<null>(`/tournaments/${id}/join`, { method: 'POST', body: {} }),
  startTournament: (id: string) =>
    request<TournamentDto>(`/tournaments/${id}/start`, { method: 'POST', body: {} }),
};
