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
cargo run --package go-fish-game-server
cargo run --package go-fish-game-server -- --config path/to/config.toml

# Run TUI client (native)
cargo run --package go-fish-tui-client
cargo run --package go-fish-tui-client -- --config path/to/config.toml

# Build TUI client for WASM (requires trunk)
trunk build
trunk serve   # dev server at localhost:8080, proxies /ws → ws://127.0.0.1:9001

# Docker (game server)
docker build -t go-fish-game-server .
docker run -p 9001:80 go-fish-game-server
```

## Architecture

This is a Rust workspace with four crates:

```
go-fish (core engine)
go-fish-web (protocol types)
go-fish-game-server (server) → depends on go-fish + go-fish-web
go-fish-tui-client (client)  → depends on go-fish + go-fish-web
```

### go-fish (core engine)

Pure game logic — no network dependencies. Core game code lives in `src/lib.rs`; bot infrastructure in `src/bots.rs`.

Key types:

| Type | Description |
|------|-------------|
| `Card` | `suit: Suit` + `rank: Rank`. Copy. |
| `Suit` | `Clubs / Diamonds / Hearts / Spades`. Implements `Sequence`. |
| `Rank` | `Two` through `Ace` (13 values). Implements `Sequence`, `Ord`, `Display`. |
| `Deck` | `Vec<Card>` with `new()`, `shuffle()`, `draw()`, `is_empty()`. |
| `IncompleteBook` | 1–3 cards of the same rank. `combine()` merges two, optionally producing a `CompleteBook`. |
| `CompleteBook` | Exactly 4 cards of the same rank (`[Card; 4]`). Copy. |
| `Hand` | `Vec<IncompleteBook>`. `add_book()` adds/combines books; `receive_hook()` yields matching cards. |
| `Player` | Active player: `id: PlayerId`, `hand: Hand`, `completed_books: Vec<CompleteBook>`. |
| `InactivePlayer` | Eliminated player: still wins if they have the most books. |
| `Hook` | A turn request: `target: PlayerId` + `rank: Rank`. |
| `Game` | Top-level game state. `player_turn` is private. |

Main API:

```rust
let deck = Deck::new().shuffle();
let mut game = Game::new(deck, player_count);
let result: Result<HookResult, TurnError> = game.take_turn(Hook { target, rank });
```

`HookResult` variants: `Catch(IncompleteBook)` — target had the rank; `GoFish` — they didn't.

Dealing: 7 cards per player for 2–3 players, 5 cards otherwise. Initial books are completed before play begins.

`advance_player_turn()` and `handle_active_player_has_empty_hand()` are the most complex methods — they temporarily remove players from `self.players` and restore order via `reorder_players()`.

**Bots** (`src/bots.rs`): `Bot` trait with `observe(BotObservation)` + `generate_hook(&[PlayerId]) -> Hook`. `SimpleBot` is the bundled implementation — a probability-table bot with configurable `memory_limit` (observations retained) and `error_margin` (Gaussian noise std-dev). See `go-fish/CLAUDE.md` for details.

### go-fish-web (protocol types)

Pure type-definition crate. Single file `src/lib.rs`. No logic, no tests. All types derive `serde::Serialize` / `serde::Deserialize`; JSON is the wire format.

**`ClientMessage` variants** (client → server):

| Variant | When sent |
|---------|-----------|
| `Identity` | First message; requests a server-assigned player name |
| `CreateLobby` | Create a new lobby |
| `JoinLobby(String)` | Join an existing lobby by ID |
| `LeaveLobby` | Exit the current lobby |
| `StartGame` | Lobby leader starts the game |
| `Hook(ClientHookRequest)` | Execute a turn (`target_name` + `rank`) |
| `AddBot { bot_type }` | Leader adds a bot slot to the lobby |
| `RemoveBot` | Leader removes the last bot slot from the lobby |

**`ServerMessage` variants** (server → client):

| Variant | When sent |
|---------|-----------|
| `PlayerIdentity(String)` | Response to `Identity` |
| `LobbyJoined { lobby_id, leader, players, max_players }` | Joining confirmed |
| `LobbyUpdated { leader, players }` | Lobby roster changed |
| `LobbyLeft(LobbyLeftReason)` | Exit confirmed |
| `GameStarted` | Game begins |
| `GameSnapshot(GameSnapshot)` | Full state sync after each turn |
| `HookError(HookError)` | Turn rejected |
| `GameResult(GameResult)` | Game over |
| `Error(String)` | Generic server error |

Serialisation round-trip tests live in `go-fish-tui-client/src/network.rs`. When adding new variants, add corresponding proptest strategies and assertions there.

### go-fish-game-server

Async tokio server. Three independent tasks communicate through typed `mpsc` channels:

```
TCP listener ──(ClientEvent)──▶ ConnectionManager ──(LobbyEvent)──▶ LobbyManager
                                        ▲                                  │
                                        └───(LobbyOutboundMessage)─────────┘
```

Module overview:

| File | Purpose |
|------|---------|
| `main.rs` | Initialises OpenTelemetry providers, parses `--config`, calls `lib::run()`, flushes telemetry on shutdown. |
| `lib.rs` | `run()` wires TCP listener + `ConnectionManager` + `LobbyManager`; handles Ctrl+C / SIGTERM. |
| `connection.rs` | WebSocket transport. Accepts connections, performs WS handshake, owns client registry, parses JSON ↔ messages, handles ping/pong keepalive (45 s interval, 3 missed = disconnect). |
| `lobby.rs` | Game-domain logic. Owns player state, lobby lifecycle, `GameSession`s. Handles identity, lobby CRUD, `StartGame`, per-turn `Hook` processing. |

`ClientPhase` state machine:

```
IdentityNegotiation ──(Identity)──▶ PreLobby ──(CreateLobby/JoinLobby)──▶ InLobby ──(StartGame/auto-start)──▶ InGame
                                        ▲              ▲                        │                                    │
                                        └──────────────┴────(LeaveLobby / game ends / disconnect)───────────────────┘
```

Key invariants: `Lobby.players[0]` is always the leader. Lobbies reset to `Waiting` after a game ends; deleted only when the last player leaves. Disconnection during a game ends the session for all players.

Configuration (TOML, defaults used if `--config` is omitted):

```toml
address = "127.0.0.1:9001"   # SocketAddr to bind
lobby_max_players = 4         # max players per lobby (auto-starts when full)
max_client_connections = 10   # hard cap; excess clients receive a Close frame

[bots]
thinking_time_min_ms = 2000   # minimum simulated thinking delay per bot turn
thinking_time_max_ms = 4500   # maximum simulated thinking delay per bot turn

[bots.simple_bot]
memory_limit = 3              # observations retained by SimpleBot (0 = memoryless)
error_margin = 0.2            # Gaussian noise std-dev applied to probability table
```

The lobby leader can add bots via `ClientMessage::AddBot { bot_type }` and remove them via `ClientMessage::RemoveBot`. Each bot runs as a `BotDriver` Tokio task that shares the same hook-processing path as human players.

`build/config.toml` is the production default bundled into the Docker image (`0.0.0.0:80`). `RUST_LOG` controls log level; standard `OTEL_*` variables configure the OTLP exporter.

### go-fish-tui-client

ratatui + crossterm TUI, also compilable to WASM (via trunk + gloo_net).

Module overview:

| File | Purpose |
|------|---------|
| `main.rs` | Entry points. Native: CLI args, `CrosstermBackend`, network task, event loop. WASM: `WebSocket` via `gloo_net`, key-event callback, `draw_web` render loop. Defines `Config`. |
| `state.rs` | All app state types and `apply_network_event`. Source of truth. |
| `event_loop.rs` | Native-only `run_event_loop`: polls crossterm (50 ms timeout), drains network channel, calls `render`. |
| `input.rs` | Platform-agnostic `KeyInput` / `Key` + `handle_key` (single dispatch for all keyboard logic). |
| `network.rs` | `NetworkEvent` enum + `run_network_task`. Multiplexes WebSocket frames ↔ channels via `tokio::select!`. |
| `ui.rs` | `render` entry point + `widgets` sub-module with all ratatui `Widget` implementations. |

`AppState` / `Screen` state machine:

```
Connecting ──(PlayerIdentity)──▶ PreLobby ──(LobbyJoined)──▶ Lobby ──(GameStarted)──▶ Game
                                     ▲                           │                        │
                                     └────────(LobbyLeft/LobbyUpdated removes player)─────┘
                                     └────────(connection closed/error from any screen)───┘
```

`GameInputState` sub-machine:

```
Idle ──[h]──▶ SelectingTarget ──[enter]──▶ SelectingRank ──[enter]──▶ sends Hook, back to Idle
               (skip if 1 opponent)         [esc] ──────────────────▶ SelectingTarget
              [esc] ──────────────────────▶ Idle
```

Vim-style navigation: `j`/`k` or arrows for target selection, `h`/`l` or arrows for rank selection. `Ctrl-C` always quits (sends `LeaveLobby` first if in a lobby).

Widgets (all in `ui.rs::widgets`, implement `Widget`, render to `Buffer`):

- **`CardWidget`** — 7×5 card, face-down or face-up with suit symbol and rank; yellow border when highlighted.
- **`TurnIndicatorWidget`** — 5×3 bordered box; filled with `█` when active.
- **`IncompleteBookWidget`** — fanned stack of face-up cards (each offset 1 column right).
- **`PlayerStripWidget`** — `Local` / `Opponent` variants. Turn indicator | cards | book count. Local is always the bottom row; opponents appear above in rotation order.

## Game Flow

1. Client connects → sends `ClientMessage::Identity` → receives `ServerMessage::PlayerIdentity`
2. Client creates or joins a lobby
3. Lobby leader sends `StartGame`
4. During play, the active player sends `Hook { target, rank }`
5. Server calls `Game::take_turn()`, broadcasts `GameSnapshot` to all clients

## Testing

- **go-fish**: `rstest` with `#[values(...)]` for parameterized unit tests in `src/game_tests.rs`; proptest integration tests in `tests/complete_games.rs` (up to 10,000 random turns for 2–6 players). `src/bots.rs` has unit tests for `SimpleBot` memory management, probability table inference, and `generate_hook` correctness.
- **go-fish-game-server**: 9+ `tokio::test` unit tests and 4 proptest suites in `connection.rs`; 18+ unit tests and 5 proptest suites in `lobby.rs`. In-memory WebSocket streams (no real TCP). Property tests run 20 iterations.
- **go-fish-tui-client**: proptest round-trip tests for `ClientMessage` / `ServerMessage` in `network.rs`; proptest state-transition tests in `state_tests.rs`; widget unit tests and a `render_game` no-panic proptest in `ui.rs`.

When adding new state transitions (server or client), add a corresponding test. When adding new `ClientMessage` / `ServerMessage` variants, add proptest strategies in `go-fish-tui-client/src/network.rs`.
