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
use nenchess_integration::{spawn_server, test_database_url, ws_url, ws_url_plain};
use protocol::{
    AppliedMoveDto, AuthResponse, ClientMessage, ErrorCode, GameHistoryResponse, GameOutcomeDto,
    GameView, MoveInputDto, ProfileDto, ServerMessage, UserDto,
};
use reqwest::StatusCode;
use serde_json::json;
use sqlx::Connection;
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

    /// GET returning the raw body and content type (for text exports).
    async fn get_text(
        &self,
        path: &str,
        token: Option<&str>,
    ) -> anyhow::Result<(StatusCode, String, String)> {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        Ok((status, content_type, response.text().await?))
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
    enqueue_as(api, token, initial_time_ms, increment_ms, false).await
}

/// Enter the *rated* queue — only rated queues are skill-gated.
async fn enqueue_rated(
    api: &Api,
    token: &str,
    initial_time_ms: i64,
    increment_ms: i64,
) -> anyhow::Result<()> {
    enqueue_as(api, token, initial_time_ms, increment_ms, true).await
}

async fn enqueue_as(
    api: &Api,
    token: &str,
    initial_time_ms: i64,
    increment_ms: i64,
    rated: bool,
) -> anyhow::Result<()> {
    let (status, body) = api
        .post(
            "/api/matchmaking",
            Some(token),
            json!({ "initial_time_ms": initial_time_ms, "increment_ms": increment_ms, "rated": rated }),
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

/// Direct database access, used only to *stage* scenarios the API cannot
/// arrange through its endpoints (seeding a rating gap, ageing a queue entry).
/// Every assertion still goes through HTTP/WS.
async fn stage_db() -> anyhow::Result<sqlx::PgConnection> {
    sqlx::PgConnection::connect(&test_database_url())
        .await
        .context("connecting to the test database for staging")
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
            }
            | ServerMessage::DrawOfferChanged {
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
        | ClientMessage::GameResign { command_id, .. }
        | ClientMessage::DrawOffer { command_id, .. }
        | ClientMessage::DrawAccept { command_id, .. }
        | ClientMessage::DrawDecline { command_id, .. } => *command_id,
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
    draw_offer_scenarios(&api, &white, &black).await?;
    pgn_history_scenarios(&api, &white, &black).await?;
    matchmaking_scenario(&api).await?;
    skill_matchmaking_scenario(&api).await?;
    ratings_scenario(&api).await?;
    tournament_scenario(&api).await?;
    ops_scenarios(&api).await?;
    cookie_scenarios(&api).await?;
    // Must run last: it deliberately exhausts the per-IP auth budget that
    // every earlier scenario (and this one) has been spending from.
    auth_rate_limit_scenario(&api).await?;

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

/// Draw offers: pending state that never bumps the version, decline and
/// withdrawal, implicit decline on a move, and settlement by agreement
/// through the same consequences path as every other ending.
async fn draw_offer_scenarios(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    let game = create_game(api, &white.token, 600_000).await?;
    let game_id = game.id;
    join_game(api, &black.token, game_id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "draw-white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "draw-black").await?;
    let white_sub = subscribe(game_id);
    white_peer.send(&white_sub).await?;
    let snapshot = expect_state(white_peer.response_to(command_id(&white_sub)).await?);
    assert_eq!(snapshot.version, 1);
    assert_eq!(snapshot.draw_offered_by, None);
    let black_sub = subscribe(game_id);
    black_peer.send(&black_sub).await?;
    expect_state(black_peer.response_to(command_id(&black_sub)).await?);

    // Accepting with nothing pending is a conflict, not a silent no-op.
    let early = ClientMessage::DrawAccept {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&early).await?;
    expect_error(
        black_peer.response_to(command_id(&early)).await?,
        ErrorCode::Conflict,
    );

    // White offers: the ack carries the new state, black learns it through
    // the outbox, and the version does not move.
    let offer = ClientMessage::DrawOffer {
        command_id: Uuid::new_v4(),
        game_id,
    };
    white_peer.send(&offer).await?;
    match white_peer.response_to(command_id(&offer)).await? {
        ServerMessage::DrawOfferChanged { offered_by, .. } => {
            assert_eq!(offered_by, Some(protocol::ColorDto::White));
        }
        other => panic!("expected draw_offer_changed, got {other:?}"),
    }
    let event = black_peer
        .next_matching(|m| {
            matches!(
                m,
                ServerMessage::DrawOfferChanged {
                    offered_by: Some(_),
                    ..
                }
            )
        })
        .await?;
    assert!(matches!(
        event,
        ServerMessage::DrawOfferChanged {
            offered_by: Some(protocol::ColorDto::White),
            ..
        }
    ));
    let http_view = get_game(api, &black.token, game_id).await?;
    assert_eq!(http_view.draw_offered_by, Some(protocol::ColorDto::White));
    assert_eq!(
        http_view.version, 1,
        "an offer is auxiliary state, not a new version"
    );

    // Black declines: cleared for everyone.
    let decline = ClientMessage::DrawDecline {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&decline).await?;
    match black_peer.response_to(command_id(&decline)).await? {
        ServerMessage::DrawOfferChanged {
            offered_by: None, ..
        } => {}
        other => panic!("expected a cleared offer, got {other:?}"),
    }
    white_peer
        .next_matching(|m| {
            matches!(
                m,
                ServerMessage::DrawOfferChanged {
                    offered_by: None,
                    ..
                }
            )
        })
        .await?;

    // A legal move implicitly declines: black offers out of turn, white
    // simply plays and the offer dies with the move transaction.
    let offer = ClientMessage::DrawOffer {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&offer).await?;
    black_peer.response_to(command_id(&offer)).await?;
    let mv = move_command(game_id, 1, "e2e4");
    white_peer.send(&mv).await?;
    let after_move = expect_state(white_peer.response_to(command_id(&mv)).await?);
    assert_eq!(after_move.version, 2);
    assert_eq!(after_move.draw_offered_by, None);

    // Settle by acceptance: white offers, black accepts, game drawn.
    let offer = ClientMessage::DrawOffer {
        command_id: Uuid::new_v4(),
        game_id,
    };
    white_peer.send(&offer).await?;
    white_peer.response_to(command_id(&offer)).await?;
    let accept = ClientMessage::DrawAccept {
        command_id: Uuid::new_v4(),
        game_id,
    };
    black_peer.send(&accept).await?;
    let ended = expect_state(black_peer.response_to(command_id(&accept)).await?);
    assert_eq!(ended.status, protocol::GameStatusDto::Finished);
    assert_eq!(ended.version, 3, "only the ending bumps the version");
    let result = ended.result.expect("result is set");
    assert_eq!(result.winner, None, "agreement is a draw");
    assert_eq!(result.reason, protocol::ResultReasonDto::Agreement);

    // Both players see the end through the outbox.
    white_peer
        .next_matching(|m| matches!(m, ServerMessage::GameEnded { .. }))
        .await?;

    // Idempotency: the earlier offer replays its stored answer even though
    // the game has finished in the meantime.
    white_peer.send(&offer).await?;
    match white_peer.response_to(command_id(&offer)).await? {
        ServerMessage::DrawOfferChanged { offered_by, .. } => {
            assert_eq!(offered_by, Some(protocol::ColorDto::White));
        }
        other => panic!("expected the stored offer to replay, got {other:?}"),
    }

    // No new offers after the end.
    let late_offer = ClientMessage::DrawOffer {
        command_id: Uuid::new_v4(),
        game_id,
    };
    white_peer.send(&late_offer).await?;
    expect_error(
        white_peer.response_to(command_id(&late_offer)).await?,
        ErrorCode::GameOver,
    );

    Ok(())
}

/// PGN export and profile history: SAN replayed from the immutable records,
/// correct headers and result token, and each caller's own outcome.
async fn pgn_history_scenarios(
    api: &Api,
    white: &AuthResponse,
    black: &AuthResponse,
) -> anyhow::Result<()> {
    // Scholar's mate so the PGN carries a real, checkmate-terminated game.
    let game = create_game(api, &white.token, 600_000).await?;
    let game_id = game.id;
    join_game(api, &black.token, game_id).await?;

    let mut white_peer = Peer::connect(&api.base, &white.token, "pgn-white").await?;
    let mut black_peer = Peer::connect(&api.base, &black.token, "pgn-black").await?;
    let white_sub = subscribe(game_id);
    white_peer.send(&white_sub).await?;
    expect_state(white_peer.response_to(command_id(&white_sub)).await?);
    let black_sub = subscribe(game_id);
    black_peer.send(&black_sub).await?;
    expect_state(black_peer.response_to(command_id(&black_sub)).await?);

    let script: [(&str, i64); 4] = [("f2f3", 1), ("e7e5", 2), ("g2g4", 3), ("d8h4", 4)];
    for (uci, version) in script {
        let command = move_command(game_id, version, uci);
        let peer = if version % 2 == 1 {
            &mut white_peer
        } else {
            &mut black_peer
        };
        peer.send(&command).await?;
        let response = peer.response_to(command_id(&command)).await?;
        if version < 4 {
            let applied = expect_accepted(response);
            assert_eq!(applied.uci, uci);
        } else {
            // The mating move settles the game through the same path as any
            // other ending: one version bump past the last move.
            let view = expect_state(response);
            assert_eq!(view.status, protocol::GameStatusDto::Finished);
            assert_eq!(view.version, 5);
        }
    }

    // PGN: headers carry both players, the time control and the decisive
    // result; the movetext is SAN with the kernel's check/mate suffixes.
    let (status, content_type, pgn) = api
        .get_text(&format!("/api/games/{game_id}/pgn"), Some(&white.token))
        .await?;
    assert_eq!(status, StatusCode::OK, "pgn export: {pgn}");
    assert!(
        content_type.starts_with("text/plain"),
        "unexpected content type: {content_type}"
    );
    assert!(
        pgn.contains(&format!("[White \"{}\"]", white.user.username)),
        "{pgn}"
    );
    assert!(
        pgn.contains(&format!("[Black \"{}\"]", black.user.username)),
        "{pgn}"
    );
    assert!(pgn.contains("[Result \"0-1\"]"), "{pgn}");
    assert!(pgn.contains("[TimeControl \"600+0\"]"), "{pgn}");
    assert!(pgn.contains("1. f3 e5 2. g4 Qh4# 0-1"), "{pgn}");

    // Profile history: newest first, the caller's own colour and outcome.
    let (status, body) = api.get("/api/me/games", Some(&black.token)).await?;
    assert_eq!(status, StatusCode::OK, "history: {body}");
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    assert_eq!(
        history.games.first().map(|entry| entry.game_id),
        Some(game_id),
        "the newest game leads black's history"
    );
    let entry = history
        .games
        .iter()
        .find(|entry| entry.game_id == game_id)
        .expect("the finished game is listed");
    assert_eq!(entry.color, protocol::ColorDto::Black);
    assert_eq!(entry.outcome, Some(GameOutcomeDto::Win));
    assert_eq!(entry.status, protocol::GameStatusDto::Finished);
    assert_eq!(entry.move_count, 4);
    assert!(!entry.rated);
    let result = entry.result.expect("finished game carries a result");
    assert_eq!(result.winner, Some(protocol::ColorDto::Black));
    assert_eq!(result.reason, protocol::ResultReasonDto::Checkmate);
    assert_eq!(
        entry
            .opponent
            .as_ref()
            .map(|opponent| opponent.username.as_str()),
        Some(white.user.username.as_str())
    );

    // The white player sees the mirror outcome.
    let (_, body) = api.get("/api/me/games", Some(&white.token)).await?;
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    let entry = history
        .games
        .iter()
        .find(|entry| entry.game_id == game_id)
        .expect("the finished game is listed");
    assert_eq!(entry.color, protocol::ColorDto::White);
    assert_eq!(entry.outcome, Some(GameOutcomeDto::Loss));

    // Waiting games are lobby material, not history.
    let waiting = create_game(api, &white.token, 600_000).await?;
    let (_, body) = api.get("/api/me/games", Some(&white.token)).await?;
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    assert!(!history
        .games
        .iter()
        .any(|entry| entry.game_id == waiting.id));

    // `limit` is clamped server-side, `offset` past the end is an empty page.
    let (_, body) = api.get("/api/me/games?limit=1", Some(&black.token)).await?;
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    assert_eq!(history.games.len(), 1);
    assert_eq!(history.games[0].game_id, game_id);
    let (_, body) = api
        .get("/api/me/games?limit=999999", Some(&black.token))
        .await?;
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    assert!(history.games.len() <= 100, "limit must be clamped");
    let (_, body) = api
        .get("/api/me/games?offset=100000", Some(&black.token))
        .await?;
    let history: GameHistoryResponse = serde_json::from_value(body)?;
    assert!(history.games.is_empty());

    // Both endpoints authenticate like every other read.
    assert_eq!(
        api.get("/api/me/games", None).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        api.get(&format!("/api/games/{game_id}/pgn"), None).await?.0,
        StatusCode::UNAUTHORIZED
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

/// Skill-based matchmaking: rated queues only pair inside an expanding rating
/// window (±150 fresh, +2 per second waited, capped at ±1000), nearest rating
/// first; casual queues ignore ratings entirely.
async fn skill_matchmaking_scenario(api: &Api) -> anyhow::Result<()> {
    let far = register(api, &TestUser::new("skill_far")).await?;
    let near = register(api, &TestUser::new("skill_near")).await?;

    // Stage a rating gap: 10+0 belongs to the `rapid` ladder, and 2000 vs
    // 1500 is far outside the fresh ±150 window.
    let mut conn = stage_db().await?;
    sqlx::query(
        "UPDATE player_ratings SET rating = 2000 WHERE user_id = $1 AND category = 'rapid'",
    )
    .bind(far.user.id)
    .execute(&mut conn)
    .await?;

    enqueue_rated(api, &far.token, 600_000, 0).await?;
    enqueue_rated(api, &near.token, 600_000, 0).await?;

    // Many scheduler ticks (50 ms each) pass with both waiting: a 500-point
    // gap must not pair inside the fresh window.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (status, body) = api.get("/api/matchmaking", Some(&near.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["in_queue"], json!(true), "{body}");
    assert_eq!(
        body["game_id"],
        serde_json::Value::Null,
        "a 500-point gap must not pair inside the fresh window: {body}"
    );

    // Ten minutes of waiting grow the window past the gap: now they pair.
    sqlx::query(
        "UPDATE matchmaking_queue SET enqueued_at = now() - interval '10 minutes' \
         WHERE user_id = $1 OR user_id = $2",
    )
    .bind(far.user.id)
    .bind(near.user.id)
    .execute(&mut conn)
    .await?;
    let far_game = await_match(api, &far.token).await?;
    let near_game = await_match(api, &near.token).await?;
    assert_eq!(far_game, near_game);

    // Nearest-first: a fresh queue pairs the two closest ratings (1500 and
    // 1510) and leaves the 2000-point outlier searching. Re-entering the queue
    // gives everyone a fresh `enqueued_at`, so the window resets to ±150.
    let mid = register(api, &TestUser::new("skill_mid")).await?;
    sqlx::query(
        "UPDATE player_ratings SET rating = 1510 WHERE user_id = $1 AND category = 'rapid'",
    )
    .bind(mid.user.id)
    .execute(&mut conn)
    .await?;
    enqueue_rated(api, &far.token, 600_000, 0).await?;
    enqueue_rated(api, &near.token, 600_000, 0).await?;
    enqueue_rated(api, &mid.token, 600_000, 0).await?;

    let near_game = await_match(api, &near.token).await?;
    let mid_game = await_match(api, &mid.token).await?;
    assert_eq!(near_game, mid_game, "the closest ratings pair first");

    // The outlier is still searching and can leave whenever it wants.
    let (status, body) = api.get("/api/matchmaking", Some(&far.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["in_queue"], json!(true), "the outlier waits: {body}");
    assert_eq!(body["game_id"], serde_json::Value::Null);
    let (status, body) = api.delete("/api/matchmaking", Some(&far.token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["in_queue"], json!(false));

    // Casual queues ignore the gap: the same two players pair immediately
    // with no window at all.
    enqueue(api, &far.token, 600_000, 0).await?;
    enqueue(api, &near.token, 600_000, 0).await?;
    let far_game = await_match(api, &far.token).await?;
    let near_game = await_match(api, &near.token).await?;
    assert_eq!(far_game, near_game);

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

// ---------------------------------------------------------------------------
// Tournament helpers
// ---------------------------------------------------------------------------

async fn create_tournament(
    api: &Api,
    token: &str,
    name: &str,
    max_participants: i32,
) -> anyhow::Result<protocol::TournamentDto> {
    let (status, body) = api
        .post(
            "/api/tournaments",
            Some(token),
            json!({
                "name": name,
                "time_control_initial_ms": 600_000,
                "time_control_increment_ms": 0,
                "rated": false,
                "max_participants": max_participants,
            }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "create tournament expected 201, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn get_tournament(
    api: &Api,
    token: &str,
    id: Uuid,
) -> anyhow::Result<protocol::TournamentView> {
    let (status, body) = api
        .get(&format!("/api/tournaments/{id}"), Some(token))
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "get tournament expected 200, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn list_tournaments(api: &Api, token: &str) -> anyhow::Result<protocol::TournamentsResponse> {
    let (status, body) = api.get("/api/tournaments", Some(token)).await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "list tournaments expected 200, got {status}: {body}"
    );
    Ok(serde_json::from_value(body)?)
}

async fn join_tournament(api: &Api, token: &str, id: Uuid) -> anyhow::Result<()> {
    let (status, body) = api
        .post(
            &format!("/api/tournaments/{id}/join"),
            Some(token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "join tournament expected 200, got {status}: {body}"
    );
    Ok(())
}

async fn start_tournament(api: &Api, token: &str, id: Uuid) -> anyhow::Result<()> {
    let (status, body) = api
        .post(
            &format!("/api/tournaments/{id}/start"),
            Some(token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::OK,
        "start tournament expected 200, got {status}: {body}"
    );
    Ok(())
}

/// Resign a tournament game on behalf of `token`, whoever they are seated as.
///
/// Returns the winner's user id so standings can be asserted without the test
/// having to know the colours the organizer was assigned.
async fn resign_tournament_game(api: &Api, token: &str, game_id: Uuid) -> anyhow::Result<Uuid> {
    let mut peer = Peer::connect(&api.base, token, "tournament-player").await?;
    let sub = subscribe(game_id);
    peer.send(&sub).await?;
    peer.response_to(command_id(&sub)).await?;
    let resign = ClientMessage::GameResign {
        command_id: Uuid::new_v4(),
        game_id,
    };
    peer.send(&resign).await?;
    let ended = peer.response_to(command_id(&resign)).await?;
    let view = expect_state(ended);
    let result = view.result.context("finished game must carry a result")?;

    let winner = match result.winner {
        Some(protocol::ColorDto::White) => view.white.as_ref().context("white winner")?.user_id,
        Some(protocol::ColorDto::Black) => view.black.as_ref().context("black winner")?.user_id,
        None => anyhow::bail!("a resignation can never be a draw"),
    };
    Ok(winner)
}

/// The standings entry for `user_id`, if present.
fn standing_for(
    view: &protocol::TournamentView,
    user_id: Uuid,
) -> Option<&protocol::ParticipantDto> {
    view.standings.iter().find(|p| p.user_id == user_id)
}

// ---------------------------------------------------------------------------
// Tournament scenario
// ---------------------------------------------------------------------------

/// Create → join → start → play → finish, all through the public API.
async fn tournament_scenario(api: &Api) -> anyhow::Result<()> {
    let organizer = register(api, &TestUser::new("trn_org")).await?;
    let one = register(api, &TestUser::new("trn_one")).await?;
    let two = register(api, &TestUser::new("trn_two")).await?;

    // An empty tournament cannot be started: there is nobody to pair.
    let lonely = create_tournament(api, &organizer.token, "Solitaire", 4).await?;
    let (status, body) = api
        .post(
            &format!("/api/tournaments/{}/start", lonely.id),
            Some(&organizer.token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::BAD_REQUEST,
        "starting a one-player tournament must be rejected, got {status}: {body}"
    );
    anyhow::ensure!(
        error_code(&body)? == ErrorCode::BadRequest,
        "expected bad_request, got {body}"
    );

    // Names are validated up front.
    let (status, _) = api
        .post(
            "/api/tournaments",
            Some(&organizer.token),
            json!({
                "name": "",
                "time_control_initial_ms": 600_000,
                "time_control_increment_ms": 0,
                "rated": false,
                "max_participants": 4,
            }),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::BAD_REQUEST,
        "an empty tournament name must be rejected, got {status}"
    );

    // A real three-player round robin.
    let created = create_tournament(api, &organizer.token, "Copa NenChess", 8).await?;
    assert_eq!(created.status, protocol::TournamentStatusDto::Registration);
    assert_eq!(created.current_round, 0);
    assert_eq!(created.total_rounds, 0);
    assert_eq!(created.created_by, organizer.user.id);

    // The creator is auto-enrolled; the two guests join.
    let view = get_tournament(api, &one.token, created.id).await?;
    assert_eq!(view.participants.len(), 1, "creator is auto-enrolled");
    join_tournament(api, &one.token, created.id).await?;
    join_tournament(api, &two.token, created.id).await?;

    let view = get_tournament(api, &organizer.token, created.id).await?;
    assert_eq!(view.participants.len(), 3);
    assert_eq!(view.rounds.len(), 0, "no rounds exist before the start");
    for participant in &view.participants {
        assert_eq!(participant.points, 0.0);
        assert_eq!(participant.games_played, 0);
        assert_eq!(
            participant.rating_snapshot, 1500,
            "fresh accounts start at 1500"
        );
    }

    // Joining twice is a conflict, and a fourth player would exceed a full
    // roster only after we fill it — here we just cover the duplicate case.
    let (status, body) = api
        .post(
            &format!("/api/tournaments/{}/join", created.id),
            Some(&one.token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::CONFLICT,
        "a duplicate join must conflict, got {status}: {body}"
    );

    // Only the organizer may start.
    let (status, _) = api
        .post(
            &format!("/api/tournaments/{}/start", created.id),
            Some(&one.token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::FORBIDDEN,
        "a participant must not start the tournament, got {status}"
    );

    // Start: three players (odd field) → the circle method pads a bye slot,
    // giving three rounds where every player sits out exactly once.
    start_tournament(api, &organizer.token, created.id).await?;
    let view = get_tournament(api, &organizer.token, created.id).await?;
    assert_eq!(
        view.tournament.status,
        protocol::TournamentStatusDto::Active
    );
    assert_eq!(view.tournament.current_round, 1);
    assert_eq!(
        view.tournament.total_rounds, 3,
        "odd fields add a bye round"
    );

    let first_round = view
        .rounds
        .iter()
        .find(|r| r.round_number == 1)
        .context("round 1 must exist")?;
    anyhow::ensure!(
        first_round.pairings.len() == 2,
        "round 1 must pair two games, got {}",
        first_round.pairings.len()
    );
    let byes = first_round.pairings.iter().filter(|p| p.bye).count();
    anyhow::ensure!(byes == 1, "one player sits out round 1 with a bye");
    let played: Vec<_> = first_round.pairings.iter().filter(|p| !p.bye).collect();
    for pairing in &played {
        anyhow::ensure!(
            pairing.game_id.is_some(),
            "every non-bye pairing must carry a game"
        );
    }

    // Starting twice is rejected while the tournament is running.
    let (status, _) = api
        .post(
            &format!("/api/tournaments/{}/start", created.id),
            Some(&organizer.token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::BAD_REQUEST,
        "restarting a running tournament must be rejected, got {status}"
    );

    // Play out round 1: whoever holds a game resigns it.
    let mut first_winners: Vec<Uuid> = Vec::new();
    for pairing in played {
        let game_id = pairing.game_id.context("non-bye pairing has a game")?;
        let (white_id, black_id) = (pairing.white_id, pairing.black_id);
        let resigner = if white_id == Some(organizer.user.id) || black_id == Some(organizer.user.id)
        {
            &organizer.token
        } else if white_id == Some(one.user.id) || black_id == Some(one.user.id) {
            &one.token
        } else {
            &two.token
        };
        first_winners.push(resign_tournament_game(api, resigner, game_id).await?);
    }

    // The bye has already scored: someone sits on a point before any game.
    let view = get_tournament(api, &organizer.token, created.id).await?;
    let leaders: Vec<_> = view.standings.iter().filter(|p| p.points > 0.0).collect();
    anyhow::ensure!(
        !leaders.is_empty(),
        "the bye recipient must already have scored a point"
    );

    // A player who both won a game and took the bye leads on two points; a
    // player who won a single game sits on one.
    for winner in &first_winners {
        let entry = standing_for(&view, *winner).context("a winner must be ranked")?;
        anyhow::ensure!(
            entry.points >= 1.0,
            "a round 1 winner must have scored, got {} for {}",
            entry.points,
            entry.username
        );
    }

    // Round 2 is open as soon as round 1 is fully decided.
    anyhow::ensure!(
        view.tournament.current_round == 2,
        "round 2 must open, got {}",
        view.tournament.current_round
    );

    // Play every remaining round: each completion opens the next one, and the
    // last decided round finishes the tournament.
    let mut current = view;
    for expected_round in 2..=current.tournament.total_rounds {
        anyhow::ensure!(
            current.tournament.current_round == expected_round,
            "round {expected_round} must be current, got {}",
            current.tournament.current_round
        );
        let round = current
            .rounds
            .iter()
            .find(|r| r.round_number == expected_round)
            .with_context(|| format!("round {expected_round} must exist"))?;
        let byes = round.pairings.iter().filter(|p| p.bye).count();
        anyhow::ensure!(byes == 1, "round {expected_round} sits one player out");

        for pairing in round.pairings.iter().filter(|p| !p.bye).collect::<Vec<_>>() {
            let game_id = pairing.game_id.context("non-bye pairing has a game")?;
            let resigner = if pairing.white_id == Some(organizer.user.id)
                || pairing.black_id == Some(organizer.user.id)
            {
                &organizer.token
            } else if pairing.white_id == Some(one.user.id) || pairing.black_id == Some(one.user.id)
            {
                &one.token
            } else {
                &two.token
            };
            resign_tournament_game(api, resigner, game_id).await?;
        }

        if expected_round < current.tournament.total_rounds {
            current = get_tournament(api, &organizer.token, created.id).await?;
        }
    }

    // Every game is decided, so the tournament is finished and the standings
    // account for all three players.
    let view = get_tournament(api, &organizer.token, created.id).await?;
    assert_eq!(
        view.tournament.status,
        protocol::TournamentStatusDto::Finished
    );
    assert!(
        view.tournament.finished_at_ms.is_some(),
        "a finished tournament records when it ended"
    );
    assert_eq!(view.standings.len(), 3, "every participant is ranked");
    for participant in &view.standings {
        anyhow::ensure!(
            participant.games_played > 0,
            "{} must have played or taken a bye",
            participant.username
        );
    }
    // Standings are sorted by descending points.
    for pair in view.standings.windows(2) {
        anyhow::ensure!(
            pair[0].points >= pair[1].points,
            "standings must be ordered by points: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }

    // The list endpoint surfaces the finished tournament to everyone.
    let listed = list_tournaments(api, &two.token).await?;
    anyhow::ensure!(
        listed.tournaments.iter().any(|t| t.id == created.id),
        "the list must include the tournament that was just played"
    );

    // A finished tournament can no longer accept registrations.
    let late = register(api, &TestUser::new("trn_late")).await?;
    let (status, _) = api
        .post(
            &format!("/api/tournaments/{}/join", created.id),
            Some(&late.token),
            json!({}),
        )
        .await?;
    anyhow::ensure!(
        status == StatusCode::BAD_REQUEST,
        "a finished tournament must refuse new entrants, got {status}"
    );

    Ok(())
}

fn command_id_of(command: &ClientMessage) -> Uuid {
    command_id(command)
}

// ---------------------------------------------------------------------------
// Ops endpoints, cookie sessions and rate limiting
// ---------------------------------------------------------------------------

/// Liveness/readiness/metrics probes and request-id propagation.
async fn ops_scenarios(api: &Api) -> anyhow::Result<()> {
    for path in ["/livez", "/readyz", "/health"] {
        let response = api.client.get(format!("{}{path}", api.base)).send().await?;
        anyhow::ensure!(
            response.status() == StatusCode::OK,
            "{path} expected 200, got {}",
            response.status()
        );
    }

    // A supplied request id is adopted and echoed…
    let response = api
        .client
        .get(format!("{}/livez", api.base))
        .header("x-request-id", "e2e-fixed-0001")
        .send()
        .await?;
    anyhow::ensure!(
        response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            == Some("e2e-fixed-0001"),
        "x-request-id must be echoed"
    );

    // …and a generated one is attached otherwise.
    let response = api.client.get(format!("{}/livez", api.base)).send().await?;
    let generated = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .context("generated x-request-id must be present")?;
    anyhow::ensure!(!generated.is_empty(), "generated x-request-id is empty");

    // Prometheus exposition with bounded route labels.
    let response = api
        .client
        .get(format!("{}/metrics", api.base))
        .send()
        .await?;
    anyhow::ensure!(
        response.status() == StatusCode::OK,
        "/metrics expected 200, got {}",
        response.status()
    );
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    anyhow::ensure!(
        content_type.starts_with("text/plain"),
        "unexpected /metrics content type {content_type}"
    );
    let text = response.text().await?;
    for expected in [
        "nenchess_up 1",
        "nenchess_build_info{version=",
        "nenchess_http_requests_total{method=\"GET\",route=\"/livez\",status=\"2xx\"}",
        "nenchess_auth_logins_total",
        "nenchess_moves_committed",
        "nenchess_outbox_published",
    ] {
        anyhow::ensure!(
            text.contains(expected),
            "/metrics is missing {expected:?}:\n{text}"
        );
    }
    Ok(())
}

/// HttpOnly cookie sessions: login sets the cookie, it authenticates REST
/// *and* the WebSocket handshake, cookie-authenticated unsafe requests are
/// held to the CSRF rules, and logout revokes it server-side.
async fn cookie_scenarios(api: &Api) -> anyhow::Result<()> {
    let user = TestUser::new("cookie");
    let client = reqwest::Client::new();

    // Register: the response carries an HttpOnly, SameSite=Lax cookie.
    let response = client
        .post(format!("{}/api/auth/register", api.base))
        .json(&json!({ "username": user.username, "password": user.password }))
        .send()
        .await?;
    anyhow::ensure!(
        response.status() == StatusCode::CREATED,
        "register expected 201, got {}",
        response.status()
    );
    let set_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("nenchess_session="))
        .context("register did not set the session cookie")?
        .to_string();
    anyhow::ensure!(
        set_cookie.contains("HttpOnly"),
        "session cookie must be HttpOnly: {set_cookie}"
    );
    anyhow::ensure!(
        set_cookie.contains("SameSite=Lax"),
        "session cookie must be SameSite=Lax: {set_cookie}"
    );
    let token = set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.strip_prefix("nenchess_session="))
        .context("cookie value missing")?;
    let cookie = format!("nenchess_session={token}");

    // The cookie alone authenticates REST.
    let me = client
        .get(format!("{}/api/auth/me", api.base))
        .header("cookie", &cookie)
        .send()
        .await?;
    anyhow::ensure!(
        me.status() == StatusCode::OK,
        "cookie must authenticate /auth/me, got {}",
        me.status()
    );
    let me_body: UserDto = me.json().await?;
    anyhow::ensure!(
        me_body.username == user.username,
        "cookie authenticated the wrong user"
    );

    let game = json!({ "initial_time_ms": 60_000, "increment_ms": 0, "rated": false });

    // CSRF: a cross-site cookie POST is rejected…
    let cross = client
        .post(format!("{}/api/games", api.base))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "cross-site")
        .json(&game)
        .send()
        .await?;
    anyhow::ensure!(
        cross.status() == StatusCode::FORBIDDEN,
        "cross-site cookie POST must be 403, got {}",
        cross.status()
    );
    let cross_body: serde_json::Value = cross.json().await?;
    anyhow::ensure!(
        cross_body["code"] == "forbidden",
        "unexpected error body {cross_body}"
    );

    // …so is a hostile Origin (older browsers without fetch metadata)…
    let evil = client
        .post(format!("{}/api/games", api.base))
        .header("cookie", &cookie)
        .header("origin", "https://evil.example")
        .json(&game)
        .send()
        .await?;
    anyhow::ensure!(
        evil.status() == StatusCode::FORBIDDEN,
        "cross-origin cookie POST must be 403, got {}",
        evil.status()
    );

    // …while the real browser posture (Sec-Fetch-Site: same-origin) works…
    let same = client
        .post(format!("{}/api/games", api.base))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "same-origin")
        .json(&game)
        .send()
        .await?;
    anyhow::ensure!(
        same.status() == StatusCode::CREATED,
        "same-origin cookie POST expected 201, got {}",
        same.status()
    );

    // …and so does an Origin that matches the request host.
    let origin_match = client
        .post(format!("{}/api/games", api.base))
        .header("cookie", &cookie)
        .header("origin", &api.base)
        .json(&game)
        .send()
        .await?;
    anyhow::ensure!(
        origin_match.status() == StatusCode::CREATED,
        "host-matching Origin expected 201, got {}",
        origin_match.status()
    );

    // Bearer requests are exempt from the CSRF rules (no cookie involved).
    let bearer = client
        .post(format!("{}/api/games", api.base))
        .header("authorization", format!("Bearer {}", cookie_token(&cookie)))
        .header("sec-fetch-site", "cross-site")
        .json(&game)
        .send()
        .await?;
    anyhow::ensure!(
        bearer.status() == StatusCode::CREATED,
        "a bearer POST with cross-site fetch metadata must still work, got {}",
        bearer.status()
    );

    // The WebSocket handshake authenticates with the cookie alone.
    let mut socket = connect_ws_cookie(&api.base, &cookie).await?;
    let ping = ClientMessage::Ping {
        command_id: Uuid::new_v4(),
    };
    socket
        .send(Message::Text(serde_json::to_string(&ping)?.into()))
        .await
        .context("could not send ping over the cookie websocket")?;
    let mut pong = false;
    for _ in 0..10 {
        let next = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .map_err(|_| anyhow::anyhow!("cookie websocket timed out"))?;
        let Some(frame) = next else {
            bail!("cookie websocket closed before pong");
        };
        let frame = frame?;
        if let Message::Text(text) = frame {
            if matches!(
                serde_json::from_str::<ServerMessage>(text.as_str())?,
                ServerMessage::Pong { .. }
            ) {
                pong = true;
                break;
            }
        }
    }
    anyhow::ensure!(pong, "no Pong over the cookie-authenticated websocket");
    let _ = socket.close(None).await;

    // Logout revokes the session server-side and expires the cookie.
    let logout = client
        .post(format!("{}/api/auth/logout", api.base))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await?;
    anyhow::ensure!(
        logout.status() == StatusCode::NO_CONTENT,
        "logout expected 204, got {}",
        logout.status()
    );
    let cleared = logout
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    anyhow::ensure!(
        cleared.contains("Max-Age=0"),
        "logout must expire the cookie: {cleared}"
    );

    let after = client
        .get(format!("{}/api/auth/me", api.base))
        .header("cookie", &cookie)
        .send()
        .await?;
    anyhow::ensure!(
        after.status() == StatusCode::UNAUTHORIZED,
        "a revoked cookie must be 401, got {}",
        after.status()
    );
    let dead = connect_ws_cookie(&api.base, &cookie).await;
    anyhow::ensure!(dead.is_err(), "a revoked cookie must not open a websocket");

    Ok(())
}

fn cookie_token(cookie: &str) -> String {
    cookie
        .strip_prefix("nenchess_session=")
        .expect("cookie header shape")
        .to_string()
}

/// Open `/ws` without a query token, authenticating with the cookie header
/// exactly as a browser does on a same-origin handshake.
async fn connect_ws_cookie(base: &str, cookie: &str) -> anyhow::Result<Ws> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = ws_url_plain(base).into_client_request()?;
    let value = cookie
        .parse()
        .map_err(|err| anyhow::anyhow!("cookie header value: {err}"))?;
    request.headers_mut().insert("cookie", value);
    let (socket, _) = connect_async(request)
        .await
        .context("cookie websocket handshake failed")?;
    Ok(socket)
}

/// Deliberately exhausts the per-IP auth budget with invalid logins.
///
/// Runs last on purpose: every earlier scenario shares the same limiter
/// window, so the exact attempt count of the first 429 varies with test
/// timing; only its arrival is asserted.
async fn auth_rate_limit_scenario(api: &Api) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    // Unknown user: the401 path is a cheap indexed lookup, no Argon2.
    let body = json!({ "username": "no_such_rate_limit_user", "password": "wrong-password" });

    let mut limited: Option<(usize, String, serde_json::Value)> = None;
    for attempt in 0..120 {
        let response = client
            .post(format!("{}/api/auth/login", api.base))
            .json(&body)
            .send()
            .await?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let payload: serde_json::Value = response.json().await?;
            limited = Some((attempt, retry_after, payload));
            break;
        }
        anyhow::ensure!(
            response.status() == StatusCode::UNAUTHORIZED,
            "unexpected login status {} on attempt {attempt}",
            response.status()
        );
    }

    let (attempt, retry_after, payload) =
        limited.context("no 429 received after 120 extra login attempts")?;
    anyhow::ensure!(!retry_after.is_empty(), "a 429 must carry Retry-After");
    anyhow::ensure!(
        payload["code"] == "rate_limited",
        "unexpected 429 body {payload}"
    );
    eprintln!("auth rate limit kicked in after {attempt} extra attempts");
    Ok(())
}
