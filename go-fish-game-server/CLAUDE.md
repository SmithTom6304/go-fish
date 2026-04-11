# CLAUDE.md — go-fish-game-server

## Commands

```bash
# Build / check
cargo build --package go-fish-game-server
cargo check --package go-fish-game-server
cargo clippy --package go-fish-game-server

# Test
cargo test --package go-fish-game-server

# Run
cargo run --package go-fish-game-server
cargo run --package go-fish-game-server -- --config path/to/config.toml

# Docker
docker build -t go-fish-game-server .
docker run -p 9001:80 go-fish-game-server
```

## Module overview

| File | Purpose |
|------|---------|
| `main.rs` | Entry point. Initialises OpenTelemetry providers (tracer + logger) before the Tokio runtime, parses `--config` CLI flag, then `block_on`s `lib::run()`. Flushes telemetry on shutdown. |
| `lib.rs` | Public API surface and `Config` struct. `run()` wires the three long-lived tasks together (TCP listener, `ConnectionManager`, `LobbyManager`) via typed mpsc channels, and handles Ctrl+C / SIGTERM. |
| `connection.rs` | WebSocket transport layer. `run_tcp_listener` accepts TCP streams and performs the WS handshake. `ConnectionManager` owns the client registry, parses JSON → `ClientMessage`, and routes outbound `ServerMessage`s. Per-client `ConnectionHandler` tasks own their `WebSocketStream` and implement ping/pong keepalive. |
| `lobby.rs` | All game-domain logic above the transport layer. `LobbyManager` owns player state, lobby lifecycle, and `GameSession`s. Handles identity negotiation, lobby CRUD, `StartGame`, and per-turn `Hook` processing. |

## Architecture

Three independent Tokio tasks communicate through typed `mpsc` channels — no shared `Mutex` in hot paths:

```
TCP listener ──(ClientEvent)──▶ ConnectionManager ──(LobbyEvent)──▶ LobbyManager
                                        ▲                                  │
                                        └───(LobbyOutboundMessage)─────────┘
```

- **TCP listener**: binds the port, accepts connections, performs the WebSocket handshake, sends `ClientEvent::Connected`
- **ConnectionManager**: owns the `HashMap<SocketAddr, ClientHandle>` registry; parses JSON; spawns one `ConnectionHandler` task per client; enforces `max_client_connections`
- **ConnectionHandler** (per-client task): `tokio::select!` over inbound WS frames, outbound `ServerMessage`s, and a 45-second ping timer
- **LobbyManager**: owns `players`, `lobbies`, `names_in_use`, and `negotiating` sets; drives the full lobby/game state machine

## State machine

`ClientPhase` tracks each connected client:

```
IdentityNegotiation ──(Identity)──▶ PreLobby ──(CreateLobby/JoinLobby)──▶ InLobby ──(StartGame/auto-start)──▶ InGame
                                        ▲              ▲                        │                                    │
                                        └──────────────┴────(LeaveLobby / game ends / disconnect)───────────────────┘
```

All transitions live in `lobby.rs::LobbyManager::handle_player_message` and `handle_event`.

Key invariants:
- `Lobby.players[0]` is always the leader.
- Lobbies are reused after a game ends (state resets to `Waiting`); they are deleted only when the last player leaves.
- Disconnection during a game ends the session for everyone (all survivors receive `GameResult` with losers).
- Player names and lobby IDs are randomly generated adjective-noun strings and are unique for the lifetime of the server process.

## Configuration

`Config` is deserialised from TOML. If `--config` is omitted or the file cannot be parsed, defaults are used.

```toml
address = "127.0.0.1:9001"   # SocketAddr to bind
lobby_max_players = 4         # max players per lobby (auto-starts when full)
max_client_connections = 10   # hard cap; excess clients receive a Close frame
```

`build/config.toml` is the production default bundled into the Docker image (`0.0.0.0:80`).

Environment variable `RUST_LOG` controls log level (default: `info`). Standard `OTEL_*` variables configure the OTLP exporter endpoint.

## Keepalive

`ConnectionHandler` sends a `Ping` frame every 45 seconds. After 3 consecutive unanswered pings (135 s) the connection is force-closed. Pong frames reset the counter.

## Testing

Tests live at the bottom of each source file.

- **`connection.rs`** — 9 `tokio::test` unit tests and 4 `proptest` suites. Use `tokio::io::duplex`-backed in-memory WebSocket streams; no real TCP.
- **`lobby.rs`** — 18+ `tokio::test` unit tests and 5 `proptest` suites. Send events directly into the manager via mpsc; no network I/O.

Property tests run 20 iterations (tuned for CI). Key properties: identity uniqueness, phase transitions, cleanup on disconnect, game session isolation.

When adding new message handlers, add a corresponding unit test and, if the handler generalises to arbitrary inputs, a property test.
