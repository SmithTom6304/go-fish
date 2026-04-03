# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build / check
cargo build --workspace
cargo check --workspace
cargo clippy --workspace

# Test
cargo test --workspace
cargo test --package go-fish                        # core engine only
cargo test --package go-fish complete_games         # integration tests

# Run server
cargo run --package go-fish-game-server -- --config <path-to-config.toml>

# Run TUI client
cargo run --package go-fish-tui-client -- --server-url ws://127.0.0.1:9001
```

## Architecture

This is a Rust workspace with four crates:

```
go-fish (core engine)
go-fish-web (protocol types)
go-fish-game-server (server) → depends on go-fish + go-fish-web
go-fish-tui-client (client)  → depends on go-fish + go-fish-web
```

**go-fish** — Pure game logic. Key types: `Card`, `Suit`, `Rank`, `Deck`, `Hand`, `Player`, `Game`. The main API is `Game::take_turn()`. No network dependencies.

**go-fish-web** — Shared protocol types serialized as JSON over WebSocket. `ClientMessage` covers identity negotiation, lobby operations, and turn actions (`Hook`). `ServerMessage` covers game state updates, hook outcomes, and lobby events.

**go-fish-game-server** — Async tokio server. Each connection gets a task; a central `LobbyManager` routes messages between connections. Configuration is TOML (default `127.0.0.1:9001`, max 4 players per lobby). Includes OpenTelemetry tracing via gRPC OTLP.

**go-fish-tui-client** — ratatui + crossterm TUI. Uses `tokio::select!` to multiplex keyboard input and WebSocket messages. State machine progresses through: `Connecting → PreLobby → Lobby → Game`. State transitions live in `state.rs::apply_network_event()`.

## Game Flow

1. Client connects → sends `ClientMessage::Identity` → receives `ServerMessage::PlayerIdentity`
2. Client creates or joins a lobby
3. Lobby leader sends `StartGame`
4. During play, the active player sends `Hook { target, rank }`
5. Server calls `Game::take_turn()`, broadcasts `HookAndResult` to all clients

## Testing

- **go-fish**: rstest for parametrized tests; proptest-based integration tests in `tests/complete_games.rs`
- **go-fish-game-server** and **go-fish-tui-client**: proptest for serialization round-trips and state invariants
