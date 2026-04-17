# go-fish-tui-client

A terminal UI client for the go-fish multiplayer card game. Connects to a `go-fish-game-server` over WebSocket and renders the game in the terminal using [ratatui](https://github.com/ratatui-org/ratatui).

The client also compiles to WebAssembly and can run in a browser via [ratzilla](https://github.com/orhun/ratzilla).

## Prerequisites

- Rust (stable, 2021 edition)
- A running `go-fish-game-server` instance (see the server crate's README)
- For WASM builds: [trunk](https://trunkrs.dev/) (`cargo install trunk`)

## Running (native)

```bash
# Connect to the default server (wss://terminaltom.com/go-fish/game-server)
cargo run --package go-fish-tui-client

# Connect to a local server
cargo run --package go-fish-tui-client -- --config config.toml
```

Example `config.toml`:

```toml
server_url = "ws://127.0.0.1:9001"
```

The `server_url` must start with `ws://` or `wss://`.

## Running (WASM / browser)

```bash
cd go-fish-tui-client
trunk serve
```

Opens a dev server at `http://localhost:8080`. WebSocket traffic to `/ws` is proxied to `ws://127.0.0.1:9001` (configured in `Trunk.toml`). The WASM build always connects to `wss://terminaltom.com/go-fish/game-server` when served from a real host.

## Controls

| Screen | Key | Action |
|--------|-----|--------|
| Pre-lobby | `c` | Create a new lobby |
| Pre-lobby | `j` | Join an existing lobby (prompts for ID) |
| Pre-lobby | `q` | Quit |
| Lobby | `s` | Start game (leader only, requires ≥ 2 participants) |
| Lobby | `a` | Add a bot slot (leader only) |
| Lobby | `d` | Remove the last bot slot (leader only) |
| Lobby | `q` | Leave lobby |
| Game (your turn) | `h` | Start a hook (select target + rank) |
| Game — selecting target | `j` / `↓` | Move cursor down |
| Game — selecting target | `k` / `↑` | Move cursor up |
| Game — selecting target | `Enter` | Confirm target |
| Game — selecting rank | `l` / `→` | Move cursor right |
| Game — selecting rank | `h` / `←` | Move cursor left |
| Game — selecting rank | `Enter` | Confirm rank and send hook |
| Game (game over) | `Enter` / `Space` | Return to pre-lobby menu |
| Anywhere | `Esc` | Cancel current input / go back |
| Anywhere | `Ctrl-C` | Quit |

## Architecture

The client is structured around four concerns:

**State** (`state.rs`) — `AppState` holds a `Screen` enum that progresses through `Connecting → PreLobby → Lobby → Game`. All screen transitions happen in `apply_network_event`, which is called whenever a `ServerMessage` arrives from the server or the connection changes.

**Networking** (`network.rs`) — A spawned async task owns the WebSocket connection. It forwards inbound frames as `NetworkEvent`s and writes outbound `ClientMessage`s as JSON text frames, using `tokio::select!` to multiplex both directions.

**Input** (`input.rs`) — `handle_key` is the single dispatch point for all keyboard logic on both native and WASM. A `GameInputState` sub-machine inside the Game screen tracks whether the player is idle, selecting a target, or selecting a rank.

**Rendering** (`ui.rs`) — `render` dispatches to a per-screen render function. The game screen builds a row of `PlayerStripWidget`s (one per player) and a notification bar. Widgets implement ratatui's `Widget` trait and write directly to the cell buffer.

## Notifications

During a game, a rolling history of the last 3 events is displayed below the player strips. Each snapshot can produce up to four kinds of notification, shown newest-first:

1. **Local book completion** — "You completed a book of Kings!" (shown newest)
2. **Hook outcome** — "You asked Bob for Kings — Go Fish!" / "Alice asked you for Twos — Caught 2 cards!"
3. **Deck draw** — "You drew a King from the deck"
4. **Opponent book completion** — "Bob completed a book of Aces!"

The local player ("you"/"You") is always rendered in green. Notifications persist across turns and are only evicted when newer ones push them beyond the history limit.

## Development

```bash
# Run all tests
cargo test --package go-fish-tui-client

# Check and lint
cargo check --package go-fish-tui-client
cargo clippy --package go-fish-tui-client
```

Tests use [proptest](https://github.com/proptest-rs/proptest) for property-based testing of state transitions, JSON serialisation round-trips, and render stability.
