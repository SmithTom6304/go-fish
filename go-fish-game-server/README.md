# go-fish-game-server

Async WebSocket game server for the Go Fish card game, built with Tokio and tokio-tungstenite.

## Features

- WebSocket-based JSON protocol (shared with `go-fish-web`)
- Lobby system: create, join, and start games; auto-starts when a lobby is full
- Supports up to N players per lobby (configurable)
- Ping/pong keepalive with automatic disconnect of unresponsive clients
- OpenTelemetry tracing and logging via OTLP (HTTP)
- TOML configuration with sensible defaults

## Quick start

```bash
# Run with defaults (127.0.0.1:9001)
cargo run --package go-fish-game-server

# Run with a config file
cargo run --package go-fish-game-server -- --config path/to/config.toml
```

## Configuration

Create a TOML file with any combination of the following fields:

```toml
address = "127.0.0.1:9001"   # interface and port to bind
lobby_max_players = 4         # players needed to auto-start a lobby
max_client_connections = 10   # hard cap on concurrent connections
```

All fields are optional; the values above are the defaults.

**Environment variables**

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log level / filter (default: `info`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Override OTLP collector endpoint |

## Docker

```bash
docker build -t go-fish-game-server .
docker run -p 9001:80 go-fish-game-server
```

The image bundles `build/config.toml` (binds `0.0.0.0:80`, 4 players, 10 connections).

## Protocol

All messages are JSON-encoded WebSocket text frames. Types are defined in the `go-fish-web` crate.

### Client → Server

| Message | When | Description |
|---|---|---|
| `"Identity"` | After connecting | Request a player identity (name) |
| `"CreateLobby"` | After identity | Create a new lobby and become its leader |
| `{"JoinLobby": "<id>"}` | After identity | Join an existing lobby by ID |
| `"StartGame"` | In lobby, leader only | Start the game (requires ≥ 2 players) |
| `{"Hook": {"target": "<name>", "rank": "<rank>"}}` | In game, your turn | Ask a player for cards of a given rank |
| `"LeaveLobby"` | In lobby or game | Leave the current lobby |

### Server → Client

| Message | Description |
|---|---|
| `{"PlayerIdentity": "<name>"}` | Assigned player name |
| `{"LobbyJoined": {...}}` | Joined a lobby; includes leader, players, and max_players |
| `{"LobbyUpdated": {...}}` | Lobby membership changed |
| `{"LobbyLeft": "<reason>"}` | Removed from lobby |
| `"GameStarted"` | Game has begun |
| `{"GameSnapshot": {...}}` | Full game state after each turn |
| `{"HookAndResult": {...}}` | Details of the last hook (broadcast to all) |
| `{"HookError": "<reason>"}` | Invalid hook attempt |
| `{"GameResult": {...}}` | Game over; includes winners and losers |
| `{"Error": "<message>"}` | Protocol or parse error |

### Game flow

```
connect → Identity → CreateLobby / JoinLobby → [StartGame] → Hook (repeat) → GameResult
```

1. Send `"Identity"` → receive `PlayerIdentity` with your assigned name.
2. Create or join a lobby.
3. The lobby leader sends `"StartGame"` (or the game auto-starts when the lobby is full).
4. On your turn, send a `Hook`. All clients receive `HookAndResult` then an updated `GameSnapshot`.
5. When the game finishes, all clients receive `GameResult` and return to `PreLobby`.

## Architecture

```
TCP listener ──(ClientEvent)──▶ ConnectionManager ──(LobbyEvent)──▶ LobbyManager
                                       ▲                                  │
                                       └──────(LobbyOutboundMessage)──────┘
```

Three independent Tokio tasks communicate through typed `mpsc` channels. One lightweight `ConnectionHandler` task is spawned per client; no mutexes are needed in the hot path.

## Testing

```bash
cargo test --package go-fish-game-server
```

The test suite includes unit tests and `proptest`-based property tests for both the connection layer (`connection.rs`) and the lobby/game layer (`lobby.rs`). Tests use in-memory duplex streams — no real network I/O — so they run in milliseconds.
