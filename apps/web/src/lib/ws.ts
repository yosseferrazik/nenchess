//! WebSocket client for live games.
//!
//! Responsibilities kept deliberately client-side and thin:
//! * express intent only (`game.move` with `expected_version` + `command_id`);
//! * apply server snapshots **idempotently by `version`** (the server
//!   publishes at-least-once through its outbox);
//! * re-subscribe automatically after a reconnect.

import { useCallback, useEffect, useRef, useState } from 'react';

import { useAuth } from './auth';
import type { ClientMessage, GameView, PieceKindDto, ServerMessage } from './protocol';

export type ConnectionStatus = 'connecting' | 'open' | 'closed';

export interface LiveState {
  game: GameView | null;
  status: ConnectionStatus;
  lastError: string | null;
  /** A command of ours is in flight. */
  pending: boolean;
}

function wsUrl(token: string): string {
  const protocol = window.location.protocol === 'https:' ? 'wss://' : 'ws://';
  return `${protocol}${window.location.host}/ws?token=${encodeURIComponent(token)}`;
}

function newCommandId(): string {
  return crypto.randomUUID();
}

export function useGameSocket(gameId: string | null): {
  state: LiveState;
  move: (from: string, to: string, promotion: PieceKindDto | null) => Promise<void>;
  resign: () => Promise<void>;
  clearError: () => void;
} {
  const { session } = useAuth();
  const socketRef = useRef<WebSocket | null>(null);
  const pendingRef = useRef(new Map<string, (message: ServerMessage) => void>());
  const [state, setState] = useState<LiveState>({
    game: null,
    status: 'closed',
    lastError: null,
    pending: false,
  });

  const apply = useCallback((message: ServerMessage) => {
    switch (message.type) {
      case 'game.state':
      case 'game.move_accepted':
      case 'game.ended': {
        setState((previous) => {
          // Snapshots may arrive twice (direct ack + outbox publication);
          // never go backwards in version.
          if (previous.game && message.game.version < previous.game.version) return previous;
          return { ...previous, game: message.game, pending: false };
        });
        break;
      }
      case 'error': {
        setState((previous) => ({
          ...previous,
          lastError: message.message,
          pending: false,
        }));
        break;
      }
      default:
        break;
    }
  }, []);

  const send = useCallback((command: ClientMessage): Promise<ServerMessage> => {
    const socket = socketRef.current;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error('connection lost'));
    }
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => {
        pendingRef.current.delete(command.command_id);
        reject(new Error('the server did not answer in time'));
      }, 15_000);
      pendingRef.current.set(command.command_id, (message) => {
        window.clearTimeout(timer);
        resolve(message);
      });
      socket.send(JSON.stringify(command));
      setState((previous) => ({ ...previous, pending: true }));
    });
  }, []);

  // Connection lifecycle: connect, re-subscribe on reopen, tear down on unmount.
  useEffect(() => {
    const token = session?.token;
    if (!token || !gameId) return;
    const pending = pendingRef.current;
    let disposed = false;
    let retry: number | undefined;

    const connect = () => {
      if (disposed) return;
      setState((previous) => ({ ...previous, status: 'connecting' }));
      const socket = new WebSocket(wsUrl(token));
      socketRef.current = socket;
      socket.onopen = () => {
        if (disposed) return;
        setState((previous) => ({ ...previous, status: 'open' }));
        void send({ type: 'game.subscribe', command_id: newCommandId(), game_id: gameId }).catch(
          () => undefined,
        );
      };
      socket.onmessage = (event) => {
        const message = JSON.parse(String(event.data)) as ServerMessage;
        // Presence events carry no command id; only correlate real answers.
        const commandId = 'command_id' in message ? message.command_id : null;
        if (commandId) {
          const waiter = pending.get(commandId);
          if (waiter) {
            pending.delete(commandId);
            waiter(message);
          }
        }
        apply(message);
      };
      socket.onclose = () => {
        // Only clear the shared ref if it still points at *this* socket: a
        // socket closed by an earlier effect cleanup must not null out the
        // newer socket that replaced it (React StrictMode mounts twice).
        if (socketRef.current === socket) {
          socketRef.current = null;
        }
        if (disposed) return;
        setState((previous) => ({ ...previous, status: 'closed', pending: false }));
        retry = window.setTimeout(connect, 1_500);
      };
      socket.onerror = () => socket.close();
    };

    connect();
    return () => {
      disposed = true;
      if (retry !== undefined) window.clearTimeout(retry);
      socketRef.current?.close();
      socketRef.current = null;
      pending.clear();
    };
  }, [session?.token, gameId, send, apply]);

  const move = useCallback(
    async (from: string, to: string, promotion: PieceKindDto | null) => {
      if (!gameId) return;
      const expected = state.game?.version;
      if (expected === undefined) return;
      const message = await send({
        type: 'game.move',
        command_id: newCommandId(),
        game_id: gameId,
        expected_version: expected,
        move: { from, to, promotion },
      });
      if (message.type !== 'error') apply(message);
    },
    [send, apply, gameId, state.game?.version],
  );

  const resign = useCallback(async () => {
    if (!gameId) return;
    const message = await send({
      type: 'game.resign',
      command_id: newCommandId(),
      game_id: gameId,
    });
    if (message.type !== 'error') apply(message);
  }, [send, apply, gameId]);

  const clearError = useCallback(() => {
    setState((previous) => ({ ...previous, lastError: null }));
  }, []);

  return { state, move, resign, clearError };
}
