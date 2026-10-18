//! End-to-end test: spawns the real API in-process against a dedicated
//! PostgreSQL database and drives it over HTTP and WebSocket, exactly as a
//! browser client would.
//!
//! It is a single `#[tokio::test]` on purpose: every scenario shares one
//! server and one database, and running scenarios sequentially keeps the
//! assertions (and failures) unambiguous.

use std::time::Duration;

use anyhow::{bail, Context};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use nenchess_integration::{spawn_server, ws_url};
use protocol::{
    AppliedMoveDto, AuthResponse, ClientMessage, ErrorCode, GameView, MoveInputDto, ProfileDto,
    ServerMessage, UserDto,
};
use reqwest::StatusCode;
use serde_json::json;
use test_support::{black_player, white_player, TestUser};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

struct Api {
    client: reqwest::Client,
    base: String,
}

impl Api {
    async fn start() -> anyhow::Result<Self> {
        let base = spawn_server().await.context("spawning the api")?;
        Ok(Self {
            client: reqwest::Client::new(),
            base,
        })
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> anyhow::Result<(StatusCode, serde_json::Value)> {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(&body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, body))
    }

    async fn get(
        &self,
        path: &str,
        token: Option<&str>,
    ) -> anyhow::Result<(StatusCode, serde_json::Value)> {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, body))
    }

    async fn delete(
        &self,
        path: &str,
        token: Option<&str>,
    ) -> anyhow::Result<(StatusCode, serde_json::Value)> {
        let mut request = self.client.delete(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, body))
    }
}

async fn register(api: &Api, user: &TestUser) -> anyhow::Result<AuthResponse> {
    let (status, body) = api
        .post(
            "/api/auth/register",
            None,
            json!({ "username": user.username, "password": user.password }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "register expected 201, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn create_game(api: &Api, token: &str, initial_time_ms: i64) -> anyhow::Result<GameView> {
    let (status, body) = api
        .post(
            "/api/games",
            Some(token),
            json!({ "initial_time_ms": initial_time_ms, "increment_ms": 0, "rated": false }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "create game expected 201, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn join_game(api: &Api, token: &str, game_id: Uuid) -> anyhow::Result<GameView> {
    let (status, body) = api
        .post(
            &format!("/api/games/{game_id}/join"),
            Some(token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "join game expected 200, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn get_game(api: &Api, token: &str, game_id: Uuid) -> anyhow::Result<GameView> {
    let (status, body) = api
        .get(&format!("/api/games/{game_id}"), Some(token))
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "get game expected 200, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn get_profile(api: &Api, token: &str) -> anyhow::Result<ProfileDto> {
    let (status, body) = api.get("/api/profile", Some(token)).await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "profile expected 200, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

/// Create a game, choosing whether it is rated.
async fn create_game_rated(
    api: &Api,
    token: &str,
    initial_time_ms: i64,
    rated: bool,
) -> anyhow::Result<GameView> {
    let (status, body) = api
        .post(
            "/api/games",
            Some(token),
            json!({ "initial_time_ms": initial_time_ms, "increment_ms": 0, "rated": rated }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "create game expected 201, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

fn error_code(body: &serde_json::Value) -> anyhow::Result<ErrorCode> {
    let code = body
        .get("code")
        .cloned()
        .context("error body without code")?;
    Ok(serde_json::from_value(code)?)
}

// ---------------------------------------------------------------------------
// Matchmaking helpers
// ---------------------------------------------------------------------------

/// Enter the queue for a time control.
async fn enqueue(
    api: &Api,
    token: &str,
    initial_time_ms: i64,
    increment_ms: i64,
) -> anyhow::Result<()> {
    let (status, body) = api
        .post(
            "/api/matchmaking",
            Some(token),
            json!({ "initial_time_ms": initial_time_ms, "increment_ms": increment_ms, "rated": false }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "enqueue expected 200, got {status}: {body}"
    );
    anyhow::ensure!(
        body["in_queue"] == json!(true),
        "expected in_queue, got {body}"
    );
    Ok(())
}

/// Poll until the server reports a paired game. Returns its id.
async fn await_match(api: &Api, token: &str) -> anyhow::Result<Uuid> {
    for _ in 0..200 {
        let (status, body) = api.get("/api/matchmaking", Some(token)).await?;
        anyhow::ensure!(
            status == StatusCode::OK,
            "status expected 200, got {status}: {body}"
        );
        if let Some(id) = body.get("game_id").and_then(|value| value.as_str()) {
            return Uuid::parse_str(id).context("game_id is not a uuid");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    bail!("no match produced within the polling window")
}

// ---------------------------------------------------------------------------
// WebSocket client
// ---------------------------------------------------------------------------

struct Peer {
    name: &'static str,
    sink: SplitSink<Ws, Message>,
    stream: SplitStream<Ws>,
}

impl Peer {
    async fn connect(base: &str, token: &str, name: &'static str) -> anyhow::Result<Self> {
        let (socket, _) = connect_async(ws_url(base, token))
            .await
            .with_context(|| format!("{name} could not open the websocket"))?;
        let (sink, stream) = socket.split();
        Ok(Self { name, sink, stream })
    }

    async fn send(&mut self, command: &ClientMessage) -> anyhow::Result<()> {
        let text = serde_json::to_string(command)?;
        self.sink
            .send(Message::Text(text.into()))
            .await
            .with_context(|| format!("{} could not send a command", self.name))?;
        Ok(())
    }

    async fn next(&mut self) -> anyhow::Result<ServerMessage> {
        let frame = tokio::time::timeout(Duration::from_secs(5), self.stream.next())
            .await
            .map_err(|_| anyhow::anyhow!("{} timed out waiting for a server message", self.name))?
            .with_context(|| format!("{}: websocket closed", self.name))??;
        match frame {
            Message::Text(text) => Ok(serde_json::from_str(text.as_str())?),
            Message::Close(reason) => bail!("{}: closed by server ({reason:?})", self.name),
            _ => bail!("{}: unexpected frame {frame:?}", self.name),
        }
    }

    /// Read frames until the predicate matches; presence events and other
    /// games' traffic are skipped.
    async fn next_matching<F>(&mut self, predicate: F) -> anyhow::Result<ServerMessage>
    where
        F: Fn(&ServerMessage) -> bool,
    {
        for _ in 0..20 {
            let message = self.next().await?;
            if predicate(&message) {
                return Ok(message);
            }
        }
        bail!("{}: no matching message within 20 frames", self.name)
    }

    /// Response to the command with this id.
    async fn response_to(&mut self, expected: Uuid) -> anyhow::Result<ServerMessage> {
        self.next_matching(|message| match message {
            ServerMessage::Pong { command_id }
            | ServerMessage::GameState { command_id, .. }
            | ServerMessage::MoveAccepted { command_id, .. } => *command_id == expected,
            ServerMessage::Error {
                command_id: Some(command_id),
                ..
            }
            | ServerMessage::GameEnded {
                command_id: Some(command_id),
                ..
            } => *command_id == expected,
            _ => false,
        })
        .await
    }
}

fn subscribe(game_id: Uuid) -> ClientMessage {
    ClientMessage::GameSubscribe {
        command_id: Uuid::new_v4(),
        game_id,
    }
}

fn move_command(game_id: Uuid, expected_version: i64, uci: &str) -> ClientMessage {
    let (from, to) = uci.split_at(2);
    let promotion = uci.chars().nth(4).map(|letter| match letter {
        'q' => protocol::PieceKindDto::Queen,
        'r' => protocol::PieceKindDto::Rook,
        'b' => protocol::PieceKindDto::Bishop,
        'n' => protocol::PieceKindDto::Knight,
        other => panic!("unsupported promotion {other}"),
    });
    ClientMessage::GameMove {
        command_id: Uuid::new_v4(),
        game_id,
        expected_version,
        mv: MoveInputDto {
            from: from.to_string(),
            to: to[..2].to_string(),
            promotion,
        },
    }
}

fn command_id(command: &ClientMessage) -> Uuid {
    match command {
        ClientMessage::Ping { command_id }
        | ClientMessage::GameSubscribe { command_id, .. }
        | ClientMessage::GameUnsubscribe { command_id, .. }
        | ClientMessage::GameMove { command_id, .. }
        | ClientMessage::GameResign { command_id, .. } => *command_id,
    }
}

fn expect_accepted(message: ServerMessage) -> AppliedMoveDto {
    match message {
        ServerMessage::MoveAccepted { applied, .. } => applied,
        other => panic!("expected MoveAccepted, got {other:?}"),
    }
}

fn expect_error(message: ServerMessage, code: ErrorCode) {
    match message {
        ServerMessage::Error { code: actual, .. } => {
            assert_eq!(actual, code, "unexpected error code")
        }
        other => panic!("expected error {code:?}, got {other:?}"),
    }
}

fn expect_state(message: ServerMessage) -> GameView {
    match message {
        ServerMessage::GameState { game, .. } => game,
        ServerMessage::MoveAccepted { game, .. } => game,
        ServerMessage::GameEnded { game, .. } => game,
        other => panic!("expected a game snapshot, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn end_to_end() -> anyhow::Result<()> {
    let api = Api::start().await?;

    let white = register(&api, &white_player()).await?;
    let black = register(&api, &black_player()).await?;

    identity_scenarios(&api, &white).await?;
    lobby_scenarios(&api, &white, &black).await?;
    live_game_scenarios(&api, &white, &black).await?;
    concurrency_scenario(&api, &white, &black).await?;
    flag_scenario(&api, &white, &black).await?;
    checkmate_scenario(&api, &white, &black).await?;
    repetition_scenario(&api, &white, &black).await?;
    spectator_scenario(&api, &white, &black).await?;
    matchmaking_scenario(&api).await?;
    ratings_scenario(&api).await?;

    Ok(())
}

/// Registration, duplicate names, login and session resolution.
async fn identity_scenarios(api: &Api, white: &AuthResponse) -> anyhow::Result<()> {
    // Duplicate username is rejected.
    let (status, body) = api
        .post(
            "/api/auth/register",
            None,
            json!({ "username": white.user.username, "password": "correct-horse-battery" }),
        )
        .await?;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate register: {body}");
    assert_eq!(error_code(&body)?, ErrorCode::Conflict);

    // Weak password / bad username are rejected before touching the database.
    let (status, _) = api
        .post(
            "/api/auth/register",
            None,
            json!({ "username": "someone", "password": "short" }),
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = api
        .post(
            "/api/auth/register",
            None,
            json!({ "username": "ab", "password": "long-enough-pass" }),
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Login works and yields a distinct session token.
    let (status, body) = api
        .post(
            "/api/auth/login",
            None,
            json!({ "username": white.user.username, "password": "correct-horse-battery" }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "login: {body}");
    let session: AuthResponse = serde_json::from_value(body)?;
    assert_ne!(session.token, white.token);
    assert_eq!(session.user.id, white.user.id);

    // Wrong password: identical error to an unknown user (no enumeration).
    let (status, body) = api
        .post(
            "/api/auth/login",
            None,
            json!({ "username": white.user.username, "password": "wrong-password" }),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body)?, ErrorCode::Unauthorized);
    let (status, _) = api
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "nosuchuser", "password": "wrong-password" }),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Session resolution.
    let (status, body) = api.get("/api/auth/me", Some(&session.token)).await?;
    assert_eq!(status, StatusCode::OK);
    let me: UserDto = serde_json::from_value(body)?;
    assert_eq!(me.id, white.user.id);
    assert_eq!(me.username, white.user.username);

    assert_eq!(
        api.get("/api/auth/me", None).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        api.get("/api/auth/me", Some("not-a-session")).await?.0,
        StatusCode::UNAUTHORIZED
    );

    // The websocket handshake authenticates through the query parameter.
    let err = connect_async(ws_url(&api.base, "not-a-session"))
        .await
        .expect_err("a bogus token must be rejected");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 401)
        }
        other => panic!("expected an HTTP 401 handshake failure, got {other:?}"),
    }

    Ok(())
}

/// Lobby: create, list, join and read; authorization on the write path.
async fn lobby_scenarios(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let created = create_game(api, &white.token, 600_000).await?;
    assert_eq!(created.status, protocol::GameStatusDto::Waiting);
    assert_eq!(created.version, 0);
    assert_eq!(created.move_count, 0);
    assert!(created.clock.is_none(), "no clock before the game starts");
    assert!(created.turn.is_none());
    assert!(created.moves.is_empty());
    assert_eq!(
        created.white.as_ref().map(|p| p.username.as_str()),
        Some(white.user.username.as_str())
    );
    assert!(created.black.is_none());

    // It shows up in the lobby.
    let (status, body) = api.get("/api/games", Some(&white.token)).await?;
    assert_eq!(status, StatusCode::OK);
    let lobby: protocol::GamesResponse = serde_json::from_value(body)?;
    assert!(lobby.games.iter().any(|g| g.id == created.id));

    // The creator cannot take the black seat.
    let (status, body) = api
        .post(
            &format!("/api/games/{}/join", created.id),
            Some(&white.token),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body)?, ErrorCode::BadRequest);

    // The second player starts the game; clocks are anchored.
    let joined = join_game(api, &black.token, created.id).await?;
    assert_eq!(joined.status, protocol::GameStatusDto::Active);
    assert_eq!(joined.version, 1);
    assert_eq!(
        joined.black.as_ref().map(|p| p.user_id),
        Some(black.user.id),
        "the joiner must be seated as black"
    );
    // Re-reading from the database must show the same seat (regression guard:
    // `black_player_id` has to be part of the persisted update).
    let reread = get_game(api, &white.token, created.id).await?;
    assert_eq!(
        reread.black.as_ref().map(|p| p.user_id),
        Some(black.user.id)
    );
    let clock = joined.clock.expect("clock present once active");
    assert_eq!(clock.white_ms, 600_000);
    assert_eq!(clock.black_ms, 600_000);
    assert_eq!(clock.turn, protocol::ColorDto::White);
    assert!(clock.server_now_ms > 0);
    assert_eq!(joined.turn, Some(protocol::ColorDto::White));

    // A started game leaves the lobby.
    let (_, body) = api.get("/api/games", Some(&white.token)).await?;
    let lobby: protocol::GamesResponse = serde_json::from_value(body)?;
    assert!(!lobby.games.iter().any(|g| g.id == created.id));

    // Third player cannot take a taken seat.
    let late = register(api, &TestUser::new("late")).await?;
    let (status, body) = api
        .post(
            &format!("/api/games/{}/join", created.id),
            Some(&late.token),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body)?, ErrorCode::GameFull);

    // Reads are open to any authenticated user; unknown games are 404.
    let fetched = get_game(api, &late.token, created.id).await?;
    assert_eq!(fetched.version, 1);
    assert_eq!(
        api.get(
            &format!("/api/games/{}", Uuid::new_v4()),
            Some(&white.token)
        )
        .await?
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        api.get(&format!("/api/games/{}", created.id), None)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );

    Ok(())
}

/// The core loop over WebSocket: subscribe, move, idempotent retry, stale
/// version, illegal move, resign and post-game rejection.
async fn live_game_scenarios(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 600_000).await?;
    let game_id = game.id;
    let joined = join_game(api, &black.token, game_id).await?;
    assert_eq!(joined.version, 1);

    let mut white_peer = Peer::connect(&api.base, &white.token, "white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "black").await?;

    // Ping/pong keeps the connection warm.
    let ping = ClientMessage::Ping {
        command_id: Uuid::new_v4(),
    };
    white_peer.send(&ping).await?;
    match white_peer.response_to(command_id(&ping)).await? {
        ServerMessage::Pong { command_id: id } => assert_eq!(id, command_id(&ping)),
        other => panic!("expected pong, got {other:?}"),
    }

    // Subscribe: the ack is the authoritative snapshot.
    let white_sub = subscribe(game_id);
    white_peer.send(&white_sub).await?;
    let snapshot = expect_state(white_peer.response_to(command_id(&white_sub)).await?);
    assert_eq!(snapshot.version, 1);

    let black_sub = subscribe(game_id);
    black_peer.send(&black_sub).await?;
    assert_eq!(
        expect_state(black_peer.response_to(command_id(&black_sub)).await?).version,
        1
    );
    // White sees black arrive (presence is best-effort, not versioned).
    let presence = white_peer
        .next_matching(|m| matches!(m, ServerMessage::PlayerJoined { .. }))
        .await?;
    match presence {
        ServerMessage::PlayerJoined { player, .. } => {
            assert_eq!(player.user_id, black.user.id)
        }
        other => panic!("expected PlayerJoined, got {other:?}"),
    }

    // White moves: accepted, version bumped, move recorded.
    let first = move_command(game_id, 1, "e2e4");
    white_peer.send(&first).await?;
    let accepted = white_peer.response_to(command_id(&first)).await?;
    let applied = expect_accepted(accepted.clone());
    assert_eq!(applied.uci, "e2e4");
    assert_eq!(applied.move_number, 1);
    let view = expect_state(accepted);
    assert_eq!(view.version, 2);
    assert_eq!(view.move_count, 1);
    assert_eq!(view.turn, Some(protocol::ColorDto::Black));
    assert!(
        view.fen.starts_with("rnbqkbnr/pppppppp/8/8/4P3"),
        "fen: {}",
        view.fen
    );
    assert_eq!(view.moves.len(), 1);

    // Black learns about it through the outbox (post-commit publication).
    let event = black_peer
        .next_matching(
            |m| matches!(m, ServerMessage::MoveAccepted { applied, .. } if applied.uci == "e2e4"),
        )
        .await?;
    assert_eq!(expect_state(event).version, 2);

    // Black replies.
    let second = move_command(game_id, 2, "e7e5");
    black_peer.send(&second).await?;
    let accepted = black_peer.response_to(command_id(&second)).await?;
    let applied = expect_accepted(accepted.clone());
    assert_eq!(applied.uci, "e7e5");
    assert_eq!(expect_state(accepted).version, 3);

    // Idempotency: resending the same command replays the original response
    // instead of applying a second move.
    white_peer.send(&first).await?;
    let replay = white_peer.response_to(command_id(&first)).await?;
    let replayed_view = expect_state(replay);
    assert_eq!(replayed_view.version, 2, "replay must not advance the game");
    assert_eq!(replayed_view.move_count, 1);
    assert_eq!(
        get_game(api, &white.token, game_id).await?.move_count,
        2,
        "the replay must not add a third move to the game"
    );

    // A fresh command with a stale version is a conflict.
    let stale = move_command(game_id, 1, "d2d4");
    white_peer.send(&stale).await?;
    expect_error(
        white_peer.response_to(command_id(&stale)).await?,
        ErrorCode::VersionConflict,
    );

    // An illegal move with the current version is rejected by the domain.
    let illegal = move_command(game_id, 3, "e2e5");
    white_peer.send(&illegal).await?;
    expect_error(
        white_peer.response_to(command_id(&illegal)).await?,
        ErrorCode::IllegalMove,
    );

    // Resignation ends the game with the correct result.
    let resign = ClientMessage::GameResign {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&resign).await?;
    let ended = black_peer.response_to(command_id(&resign)).await?;
    match &ended {
        ServerMessage::GameEnded { command_id, .. } => {
            assert_eq!(*command_id, Some(command_id_of(&resign)))
        }
        other => panic!("expected game.ended, got {other:?}"),
    }
    let view = expect_state(ended);
    assert_eq!(view.status, protocol::GameStatusDto::Finished);
    assert_eq!(view.version, 4);
    let result = view.result.expect("result is set");
    assert_eq!(result.winner, Some(protocol::ColorDto::White));
    assert_eq!(result.reason, protocol::ResultReasonDto::Resignation);

    // Both players see the end through the outbox.
    let seen = white_peer
        .next_matching(|m| matches!(m, ServerMessage::GameEnded { .. }))
        .await?;
    assert_eq!(expect_state(seen).version, 4);

    // Moves after the end are rejected.
    let after = move_command(game_id, 4, "d2d4");
    white_peer.send(&after).await?;
    expect_error(
        white_peer.response_to(command_id(&after)).await?,
        ErrorCode::GameOver,
    );

    // Idempotent resign: same command id, same answer.
    black_peer.send(&resign).await?;
    let replayed = black_peer.response_to(command_id(&resign)).await?;
    assert!(matches!(replayed, ServerMessage::GameEnded { .. }));

    // Resigning a second time with a new command is a domain error.
    let again = ClientMessage::GameResign {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&again).await?;
    expect_error(
        black_peer.response_to(command_id(&again)).await?,
        ErrorCode::GameOver,
    );

    Ok(())
}

/// Two players move at the same version: exactly one wins the race, the other
/// is told why.
async fn concurrency_scenario(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 600_000).await?;
    join_game(api, &black.token, game.id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "black").await?;

    let white_move = move_command(game.id, 1, "d2d4");
    let black_move = move_command(game.id, 1, "d7d5");

    let white_id = command_id(&white_move);
    let black_id = command_id(&black_move);

    // Fire both without waiting for the first response.
    let (white_send, black_send) =
        tokio::join!(white_peer.send(&white_move), black_peer.send(&black_move));
    white_send?;
    black_send?;

    let white_answer = white_peer.response_to(white_id).await?;
    let black_answer = black_peer.response_to(black_id).await?;

    let accepted = [&white_answer, &black_answer]
        .iter()
        .filter(|m| matches!(m, ServerMessage::MoveAccepted { .. }))
        .count();
    assert_eq!(accepted, 1, "exactly one move must be accepted");

    // The loser is rejected with a deterministic reason, not silence.
    let loser = if matches!(white_answer, ServerMessage::MoveAccepted { .. }) {
        &black_answer
    } else {
        &white_answer
    };
    match loser {
        ServerMessage::Error { code, .. } => assert!(
            matches!(code, ErrorCode::VersionConflict | ErrorCode::NotYourTurn),
            "unexpected rejection: {code:?}"
        ),
        other => panic!("expected an error for the losing move, got {other:?}"),
    }

    // The server state reflects exactly one move.
    let view = get_game(api, &white.token, game.id).await?;
    assert_eq!(view.move_count, 1);
    assert_eq!(view.version, 2);

    Ok(())
}

/// The flag falls on the server clock alone: a move that arrives after the
/// deadline ends the game instead of being applied.
async fn flag_scenario(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 1_000).await?;
    join_game(api, &black.token, game.id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "white").await?;
    tokio::time::sleep(Duration::from_millis(1_200)).await;

    let late = move_command(game.id, 1, "e2e4");
    white_peer.send(&late).await?;
    let answer = white_peer.response_to(command_id(&late)).await?;

    let view = expect_state(answer);
    assert_eq!(view.status, protocol::GameStatusDto::Finished);
    assert_eq!(view.move_count, 0, "the late move must not be recorded");
    assert_eq!(view.version, 2);
    let result = view.result.expect("result is set");
    assert_eq!(result.winner, Some(protocol::ColorDto::Black));
    assert_eq!(result.reason, protocol::ResultReasonDto::Flag);
    assert_eq!(view.clock.expect("clock").white_ms, 0);

    Ok(())
}

/// Fool's mate delivered over the wire ends the game by checkmate.
async fn checkmate_scenario(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 300_000).await?;
    join_game(api, &black.token, game.id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "black").await?;

    let sequence = ["f2f3", "e7e5", "g2g4", "d8h4"];
    let mut answer = None;
    for (version, (index, uci)) in (1..).zip(sequence.iter().enumerate()) {
        let mover = if index % 2 == 0 {
            &mut white_peer
        } else {
            &mut black_peer
        };
        let command = move_command(game.id, version, uci);
        mover.send(&command).await?;
        let response = mover.response_to(command_id(&command)).await?;
        expect_accepted(response.clone());
        answer = Some(response);
    }

    let view = expect_state(answer.expect("last move response"));
    assert_eq!(view.status, protocol::GameStatusDto::Finished);
    assert_eq!(view.move_count, 4);
    assert_eq!(view.version, 5);
    let result = view.result.expect("result is set");
    assert_eq!(result.winner, Some(protocol::ColorDto::Black));
    assert_eq!(result.reason, protocol::ResultReasonDto::Checkmate);

    Ok(())
}

/// A third user may watch and read, but never move.
async fn spectator_scenario(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 600_000).await?;
    join_game(api, &black.token, game.id).await?;

    let spectator = register(api, &TestUser::new("spec")).await?;
    let view = get_game(api, &spectator.token, game.id).await?;
    assert_eq!(view.version, 1);

    let mut peer = Peer::connect(&api.base, &spectator.token, "spectator").await?;
    let sub = subscribe(game.id);
    peer.send(&sub).await?;
    let snapshot = expect_state(peer.response_to(command_id(&sub)).await?);
    assert_eq!(snapshot.version, 1);

    // Moving is authorization-scoped to participants.
    let intrusion = move_command(game.id, snapshot.version, "e2e4");
    peer.send(&intrusion).await?;
    expect_error(
        peer.response_to(command_id(&intrusion)).await?,
        ErrorCode::Forbidden,
    );

    // The game is untouched.
    let after = get_game(api, &white.token, game.id).await?;
    assert_eq!(after.version, 1);
    assert_eq!(after.move_count, 0);

    Ok(())
}

/// A threefold repetition ends the game automatically as a draw, even though
/// neither the fifty-move counter nor the material are involved.
async fn repetition_scenario(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 600_000).await?;
    join_game(api, &black.token, game.id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "black").await?;

    // Knights shuffle out and back: the start position recurs three times.
    let sequence = [
        "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
    ];
    let mut answer = None;
    for (version, (index, uci)) in (1..).zip(sequence.iter().enumerate()) {
        let mover = if index % 2 == 0 {
            &mut white_peer
        } else {
            &mut black_peer
        };
        let command = move_command(game.id, version, uci);
        mover.send(&command).await?;
        let response = mover.response_to(command_id(&command)).await?;
        expect_accepted(response.clone());
        answer = Some(response);
    }

    let view = expect_state(answer.expect("last move response"));
    assert_eq!(view.status, protocol::GameStatusDto::Finished);
    assert_eq!(view.move_count, 8);
    assert_eq!(view.version, 9);
    let result = view.result.expect("result is set");
    assert_eq!(result.winner, None, "a repetition is a draw, not a win");
    assert_eq!(
        result.reason,
        protocol::ResultReasonDto::ThreefoldRepetition
    );

    // Further moves are refused.
    let after = move_command(game.id, 9, "g1f3");
    white_peer.send(&after).await?;
    expect_error(
        white_peer.response_to(command_id(&after)).await?,
        ErrorCode::GameOver,
    );

    Ok(())
}

/// Matchmaking: two players looking for the same time control are paired into
/// one already-active game, reported once to each of them.
async fn matchmaking_scenario(api: &Api) -> anyhow::Result<()> {
    let first = register(api, &TestUser::new("mm_a")).await?;
    let second = register(api, &TestUser::new("mm_b")).await?;

    enqueue(api, &first.token, 600_000, 0).await?;

    // Alone in the queue: no game yet.
    let (status, body) = api.get("/api/matchmaking", Some(&first.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["in_queue"], json!(true));
    assert_eq!(body["game_id"], serde_json::Value::Null);

    enqueue(api, &second.token, 600_000, 0).await?;

    // The scheduler pairs them within a few ticks; both see the same game.
    let first_game = await_match(api, &first.token).await?;
    let second_game = await_match(api, &second.token).await?;
    assert_eq!(first_game, second_game);

    // The game exists, is already active and seats the two enqueuers.
    let view = get_game(api, &first.token, first_game).await?;
    assert_eq!(view.status, protocol::GameStatusDto::Active);
    assert_eq!(
        view.white.as_ref().map(|p| p.user_id),
        Some(first.user.id),
        "the earliest entrant plays white"
    );
    assert_eq!(view.black.as_ref().map(|p| p.user_id), Some(second.user.id));

    // Consuming the match is one-shot: a later poll reports no queue entry.
    let (_, body) = api.get("/api/matchmaking", Some(&first.token)).await?;
    assert_eq!(body["in_queue"], json!(false));
    let (_, body) = api.get("/api/matchmaking", Some(&second.token)).await?;
    assert_eq!(body["in_queue"], json!(false));

    // Two players with different time controls are never paired.
    let third = register(api, &TestUser::new("mm_c")).await?;
    enqueue(api, &third.token, 600_000, 0).await?;
    enqueue(api, &first.token, 180_000, 2_000).await?;
    let (status, body) = api.get("/api/matchmaking", Some(&third.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["in_queue"],
        json!(true),
        "a different time control must not match: {body}"
    );

    // Leaving the queue is idempotent and clears the entry.
    let (status, body) = api.delete("/api/matchmaking", Some(&third.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["in_queue"], json!(false));
    let (status, body) = api.delete("/api/matchmaking", Some(&third.token)).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["in_queue"], json!(false));

    // Invalid time controls are rejected before entering the queue.
    let fourth = register(api, &TestUser::new("mm_d")).await?;
    let (status, body) = api
        .post(
            "/api/matchmaking",
            Some(&fourth.token),
            json!({ "initial_time_ms": 10, "increment_ms": 0, "rated": false }),
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body)?, ErrorCode::BadRequest);

    // Regression: `first` is still waiting alone for 3+2 (an entry nobody can be
    // paired with). It must not block two newcomers looking for 10+0.
    let fifth = register(api, &TestUser::new("mm_e")).await?;
    let sixth = register(api, &TestUser::new("mm_f")).await?;
    enqueue(api, &fifth.token, 600_000, 0).await?;
    enqueue(api, &sixth.token, 600_000, 0).await?;
    let fifth_game = await_match(api, &fifth.token).await?;
    let sixth_game = await_match(api, &sixth.token).await?;
    assert_eq!(
        fifth_game, sixth_game,
        "a lone 3+2 entry must not block a 10+0 pairing"
    );

    Ok(())
}

fn rapid_rating(profile: &ProfileDto) -> &protocol::RatingDto {
    profile
        .ratings
        .iter()
        .find(|rating| rating.category == "rapid")
        .expect("the profile always lists a rapid rating")
}

/// Elo settlement: rated games move both ratings in the same transaction as
/// the result; casual games leave them untouched.
async fn ratings_scenario(api: &Api) -> anyhow::Result<()> {
    let winner = register(api, &TestUser::new("elo_w")).await?;
    let loser = register(api, &TestUser::new("elo_l")).await?;

    // Every registered player starts with all three categories at the default.
    let fresh = get_profile(api, &winner.token).await?;
    assert_eq!(fresh.ratings.len(), 3);
    for rating in &fresh.ratings {
        assert_eq!(rating.rating, 1500, "{} starts at 1500", rating.category);
        assert_eq!(rating.games_played, 0);
    }

    // A rated 10+0 game belongs to the `rapid` ladder.
    let game = create_game_rated(api, &winner.token, 600_000, true).await?;
    assert!(game.rated);
    join_game(api, &loser.token, game.id).await?;

    // Black resigns; white wins.
    let mut peer = Peer::connect(&api.base, &loser.token, "elo-loser").await?;
    let sub = subscribe(game.id);
    peer.send(&sub).await?;
    peer.response_to(command_id(&sub)).await?;
    let resign = ClientMessage::GameResign {
        command_id: Uuid::new_v4(),
        game_id: game.id,
    };
    peer.send(&resign).await?;
    let ended = peer.response_to(command_id(&resign)).await?;
    let view = expect_state(ended);
    assert_eq!(
        view.result.expect("result is set").winner,
        Some(protocol::ColorDto::White)
    );

    // The winner gained, the loser lost, and both played exactly one rapid game.
    let winner_profile = get_profile(api, &winner.token).await?;
    let loser_profile = get_profile(api, &loser.token).await?;
    assert_eq!(rapid_rating(&winner_profile).rating, 1516);
    assert_eq!(rapid_rating(&winner_profile).games_played, 1);
    assert_eq!(rapid_rating(&loser_profile).rating, 1484);
    assert_eq!(rapid_rating(&loser_profile).games_played, 1);
    // Bullet and blitz remain untouched.
    for rating in &winner_profile.ratings {
        if rating.category != "rapid" {
            assert_eq!(
                rating.games_played, 0,
                "{} must be untouched",
                rating.category
            );
            assert_eq!(rating.rating, 1500);
        }
    }

    // A casual game is never rated.
    let casual = create_game_rated(api, &winner.token, 600_000, false).await?;
    assert!(!casual.rated);
    join_game(api, &loser.token, casual.id).await?;
    let resign_casual = ClientMessage::GameResign {
        command_id: Uuid::new_v4(),
        game_id: casual.id,
    };
    peer.send(&resign_casual).await?;
    peer.response_to(command_id(&resign_casual)).await?;

    let after_casual = get_profile(api, &winner.token).await?;
    assert_eq!(
        rapid_rating(&after_casual).rating,
        1516,
        "a casual result must not move the rating"
    );
    assert_eq!(rapid_rating(&after_casual).games_played, 1);

    Ok(())
}

fn command_id_of(command: &ClientMessage) -> Uuid {
    command_id(command)
}
