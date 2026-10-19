//! WebSocket transport for live games.
//!
//! One connection multiplexes: subscribe/unsubscribe to games, submit moves,
//! resign and receive events. Every real-time command carries a `command_id`
//! (idempotency) and moves carry `expected_version` (optimistic concurrency);
//! the client only ever expresses intent.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use game_core::{MoveInput, PieceKind, Square};
use protocol::{
    ClientMessage, ErrorCode, MoveInputDto, PieceKindDto, PlayerDto, ServerMessage, UserDto,
};

use crate::application::{moves, MoveCommand, ResignCommand};
use crate::bootstrap::AppState;
use crate::error::ApiError;
use crate::modules::{games as games_module, identity};
use crate::transport::http::session_cookie;

#[derive(Debug, Deserialize)]
pub struct WsParams {
    /// API clients authenticate with `?token=…`; browsers rely on the
    /// `nenchess_session` cookie sent with the same-origin handshake.
    token: Option<String>,
}

/// `GET /ws` — authenticates from `?token=…` or the session cookie.
pub async fn upgrade(
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let token = params
        .token
        .filter(|token| !token.is_empty())
        .or_else(|| session_cookie(&headers))
        .ok_or_else(ApiError::unauthorized)?;
    let user = identity::resolve_session(&state.db, &token)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    state
        .metrics
        .ws_connections_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(ws
        .on_upgrade(move |socket| handle_socket(state, user, socket))
        .into_response())
}

async fn handle_socket(state: AppState, user: UserDto, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let conn_id = Uuid::new_v4();
    state.metrics.ws_connections.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(user = %user.username, conn = %conn_id, "websocket connected");

    // Per-connection command budget: even without a matching game a client
    // could otherwise spam commands (each illegal move would still reach
    // the validation path).
    let command_limit = state.config.rate_limit_ws_per_minute;
    let mut command_window_start = Instant::now();
    let mut command_count = 0u32;

    loop {
        tokio::select! {
            frame = stream.next() => {
                match frame {
                    None | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if command_limit > 0 {
                            if command_window_start.elapsed() >= Duration::from_secs(60) {
                                command_window_start = Instant::now();
                                command_count = 0;
                            }
                            command_count += 1;
                            if command_count > command_limit {
                                state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                                let _ = out_tx.send(ServerMessage::Error {
                                    command_id: None,
                                    code: ErrorCode::RateLimited,
                                    message: "too many commands, slow down".to_string(),
                                });
                                continue;
                            }
                        }
                        match serde_json::from_str::<ClientMessage>(text.as_str()) {
                            Ok(command) => {
                                handle_command(&state, &user, conn_id, command, &out_tx).await;
                            }
                            Err(err) => {
                                let _ = out_tx.send(ServerMessage::Error {
                                    command_id: None,
                                    code: ErrorCode::BadRequest,
                                    message: format!("invalid command: {err}"),
                                });
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        tracing::warn!(error = %err, "websocket read error");
                        break;
                    }
                }
            }
            outbound = out_rx.recv() => {
                match outbound {
                    None => break,
                    Some(message) => {
                        let Ok(text) = serde_json::to_string(&message) else {
                            tracing::error!("failed to serialize server message");
                            continue;
                        };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Announce departure from every game this connection was watching.
    for game_id in state.hub.unsubscribe_conn(conn_id) {
        state.hub.publish(
            game_id,
            &ServerMessage::PlayerLeft {
                game_id,
                user_id: user.id,
            },
        );
    }
    state.metrics.ws_connections.fetch_sub(1, Ordering::Relaxed);
    tracing::debug!(user = %user.username, conn = %conn_id, "websocket disconnected");
}

async fn handle_command(
    state: &AppState,
    user: &UserDto,
    conn_id: Uuid,
    command: ClientMessage,
    out_tx: &mpsc::UnboundedSender<ServerMessage>,
) {
    let command_id = command_id_of(&command);
    let result: Result<Option<ServerMessage>, ApiError> = match command {
        ClientMessage::Ping { command_id } => Ok(Some(ServerMessage::Pong { command_id })),

        ClientMessage::GameSubscribe {
            command_id,
            game_id,
        } => match games_module::get_game(state, game_id).await {
            Ok(view) => {
                // Presence first (existing watchers), then register this
                // connection, then answer with the authoritative snapshot.
                state.hub.publish(
                    game_id,
                    &ServerMessage::PlayerJoined {
                        game_id,
                        player: PlayerDto {
                            user_id: user.id,
                            username: user.username.clone(),
                        },
                    },
                );
                state.hub.subscribe(game_id, conn_id, out_tx.clone());
                Ok(Some(ServerMessage::GameState {
                    command_id,
                    game: view,
                }))
            }
            Err(err) => Err(err),
        },

        ClientMessage::GameUnsubscribe { game_id, .. } => {
            state.hub.unsubscribe(game_id, conn_id);
            state.hub.publish(
                game_id,
                &ServerMessage::PlayerLeft {
                    game_id,
                    user_id: user.id,
                },
            );
            Ok(None)
        }

        ClientMessage::GameMove {
            command_id,
            game_id,
            expected_version,
            mv,
        } => match to_move_input(&mv) {
            Ok(mv) => {
                let command = MoveCommand {
                    command_id,
                    game_id,
                    expected_version,
                    mv,
                };
                moves::submit_move(state, user.id, command).await.map(Some)
            }
            Err(err) => Err(err),
        },

        ClientMessage::GameResign {
            command_id,
            game_id,
        } => {
            let command = ResignCommand {
                command_id,
                game_id,
            };
            moves::submit_resign(state, user.id, command)
                .await
                .map(Some)
        }
    };

    match result {
        Ok(Some(message)) => {
            let _ = out_tx.send(message);
        }
        Ok(None) => {}
        Err(err) => {
            let _ = out_tx.send(err.to_ws_message(command_id));
        }
    }
}

fn command_id_of(command: &ClientMessage) -> Option<Uuid> {
    match command {
        ClientMessage::Ping { command_id }
        | ClientMessage::GameSubscribe { command_id, .. }
        | ClientMessage::GameUnsubscribe { command_id, .. }
        | ClientMessage::GameMove { command_id, .. }
        | ClientMessage::GameResign { command_id, .. } => Some(*command_id),
    }
}

fn to_move_input(dto: &MoveInputDto) -> Result<MoveInput, ApiError> {
    let from: Square = dto
        .from
        .parse()
        .map_err(|_| ApiError::bad_request("invalid origin square"))?;
    let to: Square = dto
        .to
        .parse()
        .map_err(|_| ApiError::bad_request("invalid destination square"))?;
    let promotion = dto.promotion.map(|piece| match piece {
        PieceKindDto::Queen => PieceKind::Queen,
        PieceKindDto::Rook => PieceKind::Rook,
        PieceKindDto::Bishop => PieceKind::Bishop,
        PieceKindDto::Knight => PieceKind::Knight,
    });
    Ok(MoveInput {
        from,
        to,
        promotion,
    })
}
