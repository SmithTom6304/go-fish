# go-fish-web

Shared protocol types for the Go Fish game — serialised as JSON over WebSocket.

This crate defines `ClientMessage` and `ServerMessage` plus all supporting types. It contains no application logic; the server and client own the transport and deserialization call sites.

## Types

### `ClientMessage` (client → server)

| Variant | Payload | Purpose |
|---------|---------|---------|
| `Identity` | — | First message sent on connect; requests a server-assigned player name |
| `CreateLobby` | — | Create a new game lobby |
| `JoinLobby(String)` | lobby ID | Join an existing lobby |
| `LeaveLobby` | — | Exit the current lobby |
| `StartGame` | — | Lobby leader starts the game |
| `Hook(ClientHookRequest)` | `{ target_name, rank }` | Execute a turn: ask `target_name` for all cards of `rank` |

### `ServerMessage` (server → client)

| Variant | Payload | Purpose |
|---------|---------|---------|
| `PlayerIdentity(String)` | assigned name | Response to `Identity` |
| `LobbyJoined { lobby_id, leader, players, max_players }` | — | Lobby entry confirmed |
| `LobbyUpdated { leader, players }` | — | Broadcast when lobby roster changes |
| `LobbyLeft(LobbyLeftReason)` | — | Lobby exit confirmed |
| `GameStarted` | — | Broadcast when the game begins |
| `GameSnapshot(GameSnapshot)` | full state | Sent to all players after each turn |
| `HookError(HookError)` | error kind | Turn was rejected |
| `PlayerTurn(PlayerTurnValue)` | `YourTurn` or `OtherTurn(name)` | Indicates whose turn it is |
| `GameResult(GameResult)` | winners + losers | Sent when the game ends |
| `Error(String)` | message | Generic server error |

### Supporting types

**`GameSnapshot`** — the primary message during play, sent after every turn:
```rust
pub struct GameSnapshot {
    pub hand_state: HandState,                    // your hand + completed books
    pub opponents: Vec<OpponentState>,            // opponents' visible state
    pub active_player: String,                    // whose turn it is
    pub last_hook_outcome: Option<HookOutcome>,   // result of the last turn
}
```

**`OpponentState`** — what you can see of another player:
```rust
pub struct OpponentState {
    pub name: String,
    pub card_count: usize,             // hand size (individual cards hidden)
    pub completed_book_count: usize,
}
```

**`HookError`** variants: `NotYourTurn`, `UnknownPlayer(String)`, `CannotTargetYourself`, `YouDoNotHaveRank(Rank)`.

**`PlayerTurnValue`** variants: `YourTurn`, `OtherTurn(String)`.

## Wire format

All types derive `serde::Serialize` / `Deserialize`. The wire format is JSON using Serde's default tagged-union encoding:

```json
"Identity"

{"Hook": {"target_name": "Alice", "rank": "Ace"}}

{"LobbyJoined": {
  "lobby_id": "abc123",
  "leader": "Bob",
  "players": ["Alice", "Bob"],
  "max_players": 4
}}

{"GameSnapshot": {
  "hand_state": { ... },
  "opponents": [{ "name": "Alice", "card_count": 3, "completed_book_count": 1 }],
  "active_player": "Bob",
  "last_hook_outcome": null
}}
```

## Protocol flow

```
Client                          Server
  │─── Identity ───────────────▶│
  │◀── PlayerIdentity("Bob") ───│
  │
  │─── CreateLobby ────────────▶│
  │◀── LobbyJoined ─────────────│  (broadcast LobbyUpdated to others)
  │
  │─── StartGame ──────────────▶│
  │◀── GameStarted ─────────────│  (broadcast to all)
  │
  │─── Hook({target, rank}) ───▶│
  │◀── GameSnapshot ────────────│  (broadcast to all; contains HookOutcome)
  │  or HookError ──────────────│  (only to sender, if invalid)
  │
  │◀── GameResult ──────────────│  (broadcast when game ends)
```

## Dependencies

| Crate | Use |
|-------|-----|
| [`go-fish`](../go-fish) | `Rank`, `Hand`, `CompleteBook`, `HookResult`, `IncompleteBook` |
| `serde` | Serialisation derive macros |
| `serde_json` | JSON codec |

## License

MIT
