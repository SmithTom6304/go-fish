# CLAUDE.md — go-fish-tui-client

## Commands

```bash
# Build / check
cargo build --package go-fish-tui-client
cargo check --package go-fish-tui-client
cargo clippy --package go-fish-tui-client

# Test
cargo test --package go-fish-tui-client

# Run (native TUI)
cargo run --package go-fish-tui-client
cargo run --package go-fish-tui-client -- --config path/to/config.toml

# Build for WASM (requires trunk)
trunk build
trunk serve   # dev server at localhost:8080, proxies /ws → ws://127.0.0.1:9001
```

## Module overview

| File | Purpose |
|------|---------|
| `main.rs` | Entry points. Native: parses CLI args, sets up `CrosstermBackend`, starts network task, runs event loop. WASM: opens `WebSocket` via `gloo_net`, wires key-event callback and `draw_web` render loop. Also defines `Config` (deserialised from TOML). |
| `state.rs` | All app state types and the `apply_network_event` function. Source of truth for what the app knows. |
| `event_loop.rs` | Native-only `run_event_loop`: polls crossterm for key events (50 ms timeout), drains the network-event channel, calls `render`, loops until quit. |
| `input.rs` | Platform-agnostic `KeyInput` / `Key` types plus `handle_key`, which is the single dispatch point for all keyboard logic on both platforms. Returns `true` to signal quit. |
| `network.rs` | `NetworkEvent` enum plus `run_network_task` (native and WASM variants). Runs in a spawned task; multiplexes inbound WebSocket frames → `NetworkEvent` channel and outbound `ClientMessage` channel → WebSocket frames using `tokio::select!`. |
| `ui.rs` | `render` entry point (dispatches to per-screen functions) plus the `widgets` sub-module containing all ratatui `Widget` implementations. |

## State machine

`AppState` holds a single `Screen` variant:

```
Connecting ──(PlayerIdentity)──▶ PreLobby ──[j]──▶ BrowsingLobbies ──(LobbyJoined)──▶ Lobby ──(GameStarted)──▶ Game
                                     ▲                    │                                │                        │
                                     │                    └──[esc/q]────────────────────────┘                        │
                                     └────────(LobbyLeft/LobbyUpdated removes player / connection closed/error)──────┘
```

All transitions live in `state.rs::apply_network_event` (and its private helpers). Key points:

- `LobbyState.players` is `Vec<LobbyPlayer>` — each entry is either `Human { name }` or `Bot { name, bot_type }`. `LobbyUpdated` replaces the list wholesale; the UI renders bot names with their type in brackets.
- `GameSnapshot` is the primary message during play; it updates hand, opponent state, active player, and hook outcome.
- `GameResult` is stored in `GameState` but does **not** auto-navigate; the player must press Enter/Space.
- Connection errors from `Game` or `BrowsingLobbies` navigate back to `PreLobby` (player name is preserved).
- Unrecognised or out-of-context server messages are silently discarded.

### BrowsingLobbies sub-states

`BrowsingLobbiesState.status` is a `BrowsingStatus` enum with five variants:

| Variant | Meaning |
|---------|---------|
| `Loading` | Waiting for `LobbyList` response; spinner shown |
| `Loaded(Vec<LobbyInfo>)` | List received; player can select and join |
| `Creating` | Waiting for `LobbyJoined` after sending `CreateLobby`; spinner shown |
| `EnteringId { input, error }` | Player is typing a lobby ID manually |
| `Error(String)` | `RequestLobbies` failed |

`LobbyList` arriving in `BrowsingLobbies` transitions to `Loaded` and clamps `selected_index` to the new list length. Server `Error` in `EnteringId` sets the inline error field; in all other substates it transitions to `Error`.

## Input handling

`handle_key` in `input.rs` is the only place keyboard logic lives, shared between native and WASM.

**PreLobby screen:**
- `c` — sends `CreateLobby`
- `j` — transitions to `BrowsingLobbies(Loading)` and sends `RequestLobbies`
- `q` — quits

**BrowsingLobbies screen:**
- `c` (all substates except `Creating`) — transitions to `Creating`, sends `CreateLobby`
- `r` (Loading/Loaded/Error) — resets to `Loading`, sends `RequestLobbies`
- `i` (Loading/Loaded/Error) — transitions to `EnteringId`
- Up/Down or `k`/`j` (Loaded) — navigate `selected_index`
- `Enter` (Loaded, non-empty list) — sends `JoinLobby` for selected lobby
- `Esc`/`q` (Loading/Loaded/Error) — returns to `PreLobby`
- `Creating` — all keys inert while waiting for `LobbyJoined`
- `EnteringId`: char/backspace edit input; `Enter` sends `JoinLobby`; `Esc` resets to `Loading`

**Lobby screen** (leader-only actions):
- `a` — sends `AddBot { bot_type: SimpleBot }`
- `d` — sends `RemoveBot`
- `s` — sends `StartGame` (requires ≥ 2 participants)
- `q` — sends `LeaveLobby`

**Game screen** `GameInputState` sub-machine:

```
Idle ──[h]──▶ SelectingTarget ──[enter]──▶ SelectingRank ──[enter]──▶ sends Hook, back to Idle
               (skip if 1 opponent)         [esc] ──────────────────▶ SelectingTarget
              [esc] ──────────────────────▶ Idle
```

Vim-style navigation: `j`/`k` or arrow keys for target selection, `h`/`l` or arrow keys for rank selection. `Ctrl-C` always quits (sending `LeaveLobby` first if in a lobby).

## Widget system

All widgets are in `ui.rs::widgets` (a private sub-module). They implement ratatui's `Widget` trait and render directly to `Buffer`:

- **`CardWidget`** — renders a single 7×5 card, either face-down or face-up with suit symbol and rank. Highlighted variant uses yellow border.
- **`TurnIndicatorWidget`** — 5×3 bordered box; fills interior with `█` when `is_active`.
- **`IncompleteBookWidget`** — renders a fanned stack of face-up cards (each offset 1 column to the right).
- **`PlayerStripWidget`** — enum with `Local` and `Opponent` variants. Each strip is: turn indicator | cards | book count. Local shows face-up cards with optional selection highlight; Opponent shows face-down cards.

The game screen builds one `PlayerStripWidget` row per player. The local player is always rendered last (bottom row); opponents appear above in rotation order (`strip_order` helper).

- **`NotificationWidget<N: IntoNotificationLine>`** — renders a `VecDeque<N>` as a `Paragraph`, newest-first or oldest-first depending on `NotificationOrder`. `IntoNotificationLine` is implemented for both `String` (unstyled) and `Line<'static>` (preserves spans).

## Notifications

`GameState.notifications` is a `VecDeque<Line<'static>>` capped at `MAX_NOTIFICATION_HISTORY` (3). Each `GameSnapshot` may push up to four kinds of entry via `push_front` + `truncate`; the oldest are evicted when the cap is exceeded. Notifications persist across snapshots until evicted.

Addition order per snapshot (last-pushed = newest = displayed at top):

| # | Event | Example |
|---|-------|---------|
| 1 (oldest) | Opponent book completion | `Bob completed a book of Aces!` |
| 2 | Hook outcome | `You asked Bob for Kings — Go Fish!` |
| 3 | Deck draw | `You drew a King from the deck` |
| 4 (newest) | Local book completion | `You completed a book of Kings!` |

Deck draw and local book completions are suppressed on the first snapshot (initial deal). The local player is always represented as `"You"` / `"you"` and rendered in green (`Color::Green`) via styled `Span`s built at notification-construction time in `state.rs`.

## Testing

- **`state_tests.rs`** — proptest-based tests for `apply_network_event`. Covers all `Screen::Game` message handlers (Properties 11–19), verifying state transitions and that game-only messages are discarded outside the Game screen. Includes `BrowsingLobbies` transition tests: `LobbyList` → `Loaded`, `selected_index` clamping, `Error` routing, `LobbyJoined` from browsing, and connection-closed navigation. Includes a `lobby_player_strategy` that generates both `Human` and `Bot` variants.
- **`input.rs` tests** — unit tests for key handling across all `BrowsingLobbies` substates: navigation (`j`/`k`, arrows), `c`/`r`/`i`/`Esc`/`q` transitions, `EnteringId` char input, and guard for empty list `Enter`.
- **`network.rs` tests** — proptest round-trip tests for `ClientMessage` and `ServerMessage` JSON serialisation, including `RequestLobbies` and `LobbyList(Vec<LobbyInfo>)`. Includes strategies for `BotType`, `LobbyPlayer`, `LobbyInfo`, `AddBot`, and `RemoveBot`.
- **`ui.rs` render tests** — unit tests for individual widgets (`CardWidget`, `TurnIndicatorWidget`, `IncompleteBookWidget`), and proptests that `render_game` and `render_browsing_lobbies` never panic across arbitrary state values.

When adding new state transitions, add a corresponding property test in `state_tests.rs`. When adding new widgets, add unit tests in `ui.rs::widgets::tests`.
