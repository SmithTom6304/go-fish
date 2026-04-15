# CLAUDE.md — go-fish-web

## Commands

```bash
# Build / check
cargo build --package go-fish-web
cargo check --package go-fish-web
cargo clippy --package go-fish-web

# No tests live in this crate — serialisation round-trip tests are in go-fish-tui-client
cargo test --package go-fish-tui-client   # exercises ClientMessage / ServerMessage via proptest
```

## Module overview

The entire crate is a single file:

| File | Purpose |
|------|---------|
| `src/lib.rs` | All public protocol types — no logic, no tests |

This is a pure type-definition crate. No application logic lives here. Consumers (server, TUI client) own all serialisation/deserialisation call sites.

## Public types

### Messages

| Type | Direction | Purpose |
|------|-----------|---------|
| `ClientMessage` | client → server | All messages a client can send |
| `ServerMessage` | server → client | All messages a server can send |

**`ClientMessage` variants**

| Variant | Payload | When sent |
|---------|---------|-----------|
| `Identity` | — | First message; requests a server-assigned player name |
| `CreateLobby` | — | Create a new lobby |
| `JoinLobby(String)` | lobby ID | Join an existing lobby |
| `LeaveLobby` | — | Exit the current lobby |
| `StartGame` | — | Lobby leader starts the game |
| `Hook(ClientHookRequest)` | target name + rank | Execute a turn |
| `AddBot { bot_type }` | `BotType` | Leader adds a bot slot to the lobby |
| `RemoveBot` | — | Leader removes the last bot slot from the lobby |

**`ServerMessage` variants**

| Variant | Payload | When sent |
|---------|---------|-----------|
| `PlayerIdentity(String)` | assigned name | Response to `Identity` |
| `LobbyJoined { lobby_id, leader, players, max_players }` | `players: Vec<LobbyPlayer>` | Joining confirmed |
| `LobbyUpdated { leader, players }` | `players: Vec<LobbyPlayer>` | Broadcast when lobby roster changes |
| `LobbyLeft(LobbyLeftReason)` | — | Exit confirmed |
| `GameStarted` | — | Broadcast when game begins |
| `GameSnapshot(GameSnapshot)` | full state | Sent after each turn to all players |
| `HookAndResult(HookAndResult)` | request + outcome | Embedded in `GameSnapshot` |
| `HookError(HookError)` | error kind | Turn rejected |
| `HandState(HandState)` | hand + completed books | (legacy / direct hand update) |
| `PlayerTurn(PlayerTurnValue)` | `YourTurn` or `OtherTurn(name)` | Indicates active player |
| `GameResult(GameResult)` | winners + losers | Game over |
| `Error(String)` | message | Generic server error |

### Supporting types

| Type | Description |
|------|-------------|
| `BotType` | `SimpleBot` — identifies which bot implementation to add |
| `LobbyPlayer` | `Human { name }` or `Bot { name, bot_type }` — a slot in the lobby player list |
| `ClientHookRequest` | `{ target_name: String, rank: Rank }` — client turn payload |
| `FullHookRequest` | Adds `fisher_name` for server-side processing |
| `HookAndResult` | `FullHookRequest` + `HookResult` (from `go-fish`) |
| `HookError` | `NotYourTurn`, `UnknownPlayer(String)`, `CannotTargetYourself`, `YouDoNotHaveRank(Rank)` |
| `HookOutcome` | fisher + target + rank + `HookResult` — embeds in `GameSnapshot` |
| `HandState` | Player's `Hand` + `Vec<CompleteBook>` (from `go-fish`) |
| `OpponentState` | `{ name, card_count, completed_books }` — opponents' visible state only |
| `GameSnapshot` | `{ hand_state, opponents, active_player, last_hook_outcome, deck_size }` — full per-turn sync |
| `GameResult` | `{ winners: Vec<String>, losers: Vec<String> }` |
| `PlayerTurnValue` | `YourTurn` \| `OtherTurn(String)` |
| `LobbyLeftReason` | `RequestedByPlayer` |

## Serialisation

All types derive `serde::Serialize` / `serde::Deserialize`. No custom serialisation logic.

JSON is the wire format. Serde's default tagged-union encoding applies to enums:
```json
"Identity"
{"Hook": {"target_name": "Alice", "rank": "Ace"}}
{"LobbyJoined": {"lobby_id": "abc", "leader": "Bob", "players": ["Bob"], "max_players": 4}}
{"GameSnapshot": { ... }}
```

## Dependencies

| Crate | Use |
|-------|-----|
| `go-fish` | Re-uses `Rank`, `Hand`, `CompleteBook`, `HookResult`, `IncompleteBook` |
| `serde` | Derive macros |
| `serde_json` | JSON support for consumers |

## Testing

This crate has no tests of its own. Proptest-based serialisation round-trip tests for all `ClientMessage` and `ServerMessage` variants live in `go-fish-tui-client/src/network.rs`. When adding new variants, add corresponding strategies and round-trip assertions there.
