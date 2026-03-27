# Go Fish Game Server — Design

## Overview

This document consolidates the design for the `go-fish-game-server` crate across two feature areas:

1. **Connection Management** — the WebSocket transport layer built on Tokio and tokio-tungstenite.
2. **Lobby and Game** — identity negotiation, lobby lifecycle, and Go Fish game sessions layered on top.

The connection layer establishes the core pattern: **per-connection tasks communicating with a central controller via mpsc channels**. The lobby and game layer introduces a second manager (`LobbyManager`) that sits alongside the `ConnectionManager` and owns all application logic. Neither manager replaces the other; they communicate via dedicated channel pairs.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        go-fish-game-server                      │
│                                                                 │
│  TCP Listener Task                                              │
│       │ ClientEvent::Connected                                  │
│       ▼                                                         │
│  ConnectionManager Task ──── LobbyEvent ────► LobbyManager Task│
│       │  ▲                ◄─ LobbyOutboundMessage ──────────────┘
│       │  │                                                      │
│  ConnectionHandler   ConnectionHandler   ConnectionHandler ...  │
│  (one per client)                                               │
└─────────────────────────────────────────────────────────────────┘
```

### Task Topology

| Task | Cardinality | Responsibility |
|---|---|---|
| TCP Listener | 1 | Binds port, accepts TCP streams, performs WS handshake, sends `ClientEvent::Connected` to `ConnectionManager` |
| ConnectionManager | 1 | Owns client registry, receives `ClientEvent`s, parses raw text into `ClientMessage`, forwards `LobbyEvent`s to `LobbyManager`, delivers `LobbyOutboundMessage`s to `ConnectionHandler`s |
| ConnectionHandler | 1 per client | Owns `WebSocketStream`, reads frames → `ClientEvent`, writes `ServerMessage` → WS frames |
| LobbyManager | 1 | Owns player registry, lobbies, and game sessions; processes `LobbyEvent`s; sends `LobbyOutboundMessage`s back to `ConnectionManager` |

### Channel Layout

```
TCP Listener  ──(mpsc: ClientEvent)──►  ConnectionManager
                                              │  ▲
                          (mpsc: LobbyEvent) ▼  │ (mpsc: LobbyOutboundMessage)
                                         LobbyManager
                                              
ConnectionManager ──(mpsc: ServerMessage, per-client)──► ConnectionHandler(s)
ConnectionHandler(s) ──(mpsc: ClientEvent, shared)──► ConnectionManager
```

- One **shared inbound channel** (`mpsc::Sender<ClientEvent>`) is cloned into every `ConnectionHandler` and the TCP listener.
- One **per-client outbound channel** (`mpsc::Sender<ServerMessage>`) is created when a connection is registered. `ConnectionManager` holds the sender; `ConnectionHandler` holds the receiver.
- One **`ConnectionManager` → `LobbyManager`** channel carries `LobbyEvent`s.
- One **`LobbyManager` → `ConnectionManager`** channel carries `LobbyOutboundMessage`s. `ConnectionManager` polls this in its `select!` loop.
- Separate **command channels** (`mpsc::Sender<ManagerCommand>` / `mpsc::Sender<LobbyCommand>`) allow external shutdown of each component independently.

---

## Module Structure

```
go-fish-game-server/src/
├── lib.rs          — Config, top-level run(), wires ConnectionManager + LobbyManager
├── connection.rs   — ConnectionManager, ConnectionHandler, all connection types
└── lobby.rs        — LobbyManager, all lobby/game types
```

---

## Client State Machine

```
[WebSocket connected]
        │
        ▼
Identity_Negotiation_Phase
        │  Identity message received
        │  → PlayerIdentity sent
        ▼
    Pre_Lobby ◄──────────────────────────────────────────────┐
        │                                                     │
        │ CreateLobby / JoinLobby                             │
        ▼                                                     │
     In_Lobby ──── StartGame (leader, ≥2) ──► In_Game ───────┘
        │          or lobby reaches max_players    │  game ends normally
        │                                          │  or player disconnects (survivors)
        │ LeaveLobby / last player leaves          │
        └──────────────────────────────────────────┘
```

### Valid Transitions

| From | Event | To | Notes |
|---|---|---|---|
| Identity_Negotiation_Phase | `Identity` message | Pre_Lobby | Assigns random 5-char name; sends `PlayerIdentity` |
| Identity_Negotiation_Phase | Any other message | Identity_Negotiation_Phase | Sends `Error`; stays in phase |
| Identity_Negotiation_Phase | Disconnect | — | Resources released; no player state created |
| Pre_Lobby | `CreateLobby` | In_Lobby | Creates lobby; player becomes leader |
| Pre_Lobby | `JoinLobby(id)` | In_Lobby | Joins existing lobby if exists and not full |
| Pre_Lobby | Disconnect | — | Removed from registry |
| In_Lobby | `LeaveLobby` | Pre_Lobby | Leadership transferred if needed; lobby closed if empty |
| In_Lobby | `StartGame` (leader, ≥2 players) | In_Game | Game session started |
| In_Lobby | Lobby reaches `max_players` | In_Game | Auto-start |
| In_Lobby | Disconnect | Pre_Lobby (others) | Same as LeaveLobby rules |
| In_Game | Game ends normally | Pre_Lobby | `GameResult` sent to all |
| In_Game | `LeaveLobby` | — | Rejected with `Error` |
| In_Game | Disconnect | Pre_Lobby (survivors) | Game ended; `GameResult` sent to survivors |

---

## Components and Interfaces

### `Config`

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub address: SocketAddr,
    pub lobby_max_players: usize,  // must be >= 2
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "127.0.0.1:9001".parse().unwrap(),
            lobby_max_players: 4,
        }
    }
}
```

### Public Entry Point

```rust
// lib.rs
pub async fn run(config: Config) -> Result<(), anyhow::Error>;
```

`run` creates both managers, wires their channels, spawns the TCP listener task, and races the managers' event loops against `tokio::signal::ctrl_c()`. On `Ctrl+C` it sends `Shutdown` to both the listener and the `ConnectionManager`.

---

### `ConnectionManager` (`connection.rs`)

The central transport coordinator. Runs as a single long-lived Tokio task.

```rust
pub struct ConnectionManager<S = tokio::net::TcpStream> {
    clients:           HashMap<SocketAddr, ClientHandle>,
    event_rx:          mpsc::Receiver<ClientEvent<S>>,
    event_tx:          mpsc::Sender<ClientEvent<S>>,
    command_rx:        mpsc::Receiver<ManagerCommand>,
    command_tx:        mpsc::Sender<ManagerCommand>,
    lobby_tx:          mpsc::Sender<LobbyEvent>,
    lobby_outbound_rx: mpsc::Receiver<LobbyOutboundMessage>,
}

struct ClientHandle {
    tx: mpsc::Sender<ServerMessage>,
}
```

Event loop behaviour:

| Event | Action |
|---|---|
| `ClientEvent::Connected` | Register client, spawn `ConnectionHandler`, send `LobbyEvent::ClientConnected` |
| `ClientEvent::Message { text }` | Parse JSON → `ClientMessage`; on success send `LobbyEvent::ClientMessage`; on failure send `ServerMessage::Error("invalid message")` directly |
| `ClientEvent::Disconnected` | Remove from registry, send `LobbyEvent::ClientDisconnected` |
| `lobby_outbound_rx` message | Look up address in `clients`, deliver `ServerMessage` via `ClientHandle` |
| `ManagerCommand::Shutdown` | Send `ServerMessage::Disconnect` to all clients, exit loop |

### `ConnectionHandler` (`connection.rs`)

One per connected client. Owns the `WebSocketStream`.

```rust
pub struct ConnectionHandler {
    address:  SocketAddr,
    ws:       WebSocketStream<TcpStream>,
    event_tx: mpsc::Sender<ClientEvent>,
    msg_rx:   mpsc::Receiver<ServerMessage>,
}
```

Runs a `tokio::select!` loop:
- `ws.next()` arm: reads a frame, converts to `ClientEvent`, sends to `ConnectionManager`.
- `msg_rx.recv()` arm: receives `ServerMessage` from `ConnectionManager`, writes to WebSocket.

Frame handling:

| Frame | Action |
|---|---|
| Text | Send `ClientEvent::Message` |
| Close | Send `ClientEvent::Disconnected { reason: Clean }`, break |
| Binary / Ping / Pong | Ignore, continue |
| Stream error | Log ERROR, send `ClientEvent::Disconnected { reason: Error(...) }`, break |
| Stream `None` (force close) | Log event, send `ClientEvent::Disconnected { reason: ForceClosed }`, break |
| `msg_rx` returns `None` | Exit task cleanly (handle dropped) |
| `ServerMessage::Disconnect` | Send WebSocket Close frame, break |

### TCP Listener task (`connection.rs`)

A standalone async function. Binds the port, loops on `listener.accept()`. For each accepted stream performs the WebSocket handshake and sends `ClientEvent::Connected` to `ConnectionManager`. Listens on its own `mpsc::Receiver<ManagerCommand>` and stops on `Shutdown`.

---

### `LobbyManager` (`lobby.rs`)

Owns all application state. Runs as a single long-lived Tokio task.

```rust
pub struct LobbyManager {
    negotiating:       HashSet<SocketAddr>,
    players:           HashMap<SocketAddr, PlayerRecord>,
    names_in_use:      HashSet<String>,
    lobbies:           HashMap<String, Lobby>,
    lobby_max_players: usize,
    event_rx:          mpsc::Receiver<LobbyEvent>,
    outbound_tx:       mpsc::Sender<LobbyOutboundMessage>,
    command_rx:        mpsc::Receiver<LobbyCommand>,
}
```

`LobbyManager::run` loops on `tokio::select!` over `event_rx` and `command_rx`, dispatching to handler methods.

---

## Data Models

### Connection-Layer Types (`connection.rs`)

```rust
/// Events flowing from ConnectionHandlers / TCP Listener → ConnectionManager
pub enum ClientEvent<S = TcpStream> {
    Connected {
        address: SocketAddr,
        tx: mpsc::Sender<ServerMessage>,
        ws: WebSocketStream<S>,
    },
    Message {
        address: SocketAddr,
        text: String,
    },
    Disconnected {
        address: SocketAddr,
        reason: DisconnectReason,
    },
}

pub enum DisconnectReason {
    Clean,
    ForceClosed,
    Error(String),
}

/// Messages flowing from ConnectionManager → ConnectionHandlers
pub enum ServerMessage {
    Text(String),
    Disconnect,
}

/// Server-level control signals
pub enum ManagerCommand {
    Shutdown,
}
```

### Lobby-Layer Types (`lobby.rs`)

```rust
/// Events flowing from ConnectionManager → LobbyManager
pub enum LobbyEvent {
    ClientConnected    { address: SocketAddr },
    ClientMessage      { address: SocketAddr, message: go_fish_web::ClientMessage },
    ClientDisconnected { address: SocketAddr, reason: DisconnectReason },
}

pub enum LobbyCommand {
    Shutdown,
}

/// Messages flowing from LobbyManager → ConnectionManager
pub struct LobbyOutboundMessage {
    pub address: SocketAddr,
    pub message: go_fish_web::ServerMessage,
}

pub enum ClientPhase {
    IdentityNegotiation,
    PreLobby,
    InLobby { lobby_id: String },
    InGame  { lobby_id: String },
}

pub struct PlayerRecord {
    pub name:    String,
    pub address: SocketAddr,
    pub phase:   ClientPhase,
}

pub struct Lobby {
    pub lobby_id:    String,
    pub players:     Vec<SocketAddr>,  // players[0] is always the leader
    pub max_players: usize,
    pub state:       LobbyState,
}

pub enum LobbyState {
    Waiting,
    InGame(GameSession),
}

pub struct GameSession {
    pub game:         go_fish::Game,
    pub id_to_name:   HashMap<go_fish::PlayerId, String>,
    pub name_to_id:   HashMap<String, go_fish::PlayerId>,
    pub name_to_addr: HashMap<String, SocketAddr>,
}
```

### Application Message Types (`go-fish-web`)

```rust
pub enum ClientMessage {
    Hook(ClientHookRequest),
    Identity,
    CreateLobby,
    JoinLobby(String),   // lobby_id
    LeaveLobby,
    StartGame,
}

pub enum ServerMessage {
    HookAndResult(HookAndResult),
    HookError(HookError),
    HandState(HandState),        // hand + completed books
    PlayerTurn(PlayerTurnValue),
    PlayerIdentity(String),
    GameResult(GameResult),
    LobbyJoined {
        lobby_id:    String,
        leader:      String,
        players:     Vec<String>,
        max_players: usize,
    },
    LobbyUpdated {
        leader:  String,
        players: Vec<String>,
    },
    GameStarted,
    Error(String),
}
```

Note: `HandState` was renamed from `PlayerState` in `go-fish-web` to avoid a name collision with the `PlayerState`/`ClientPhase` enum in `lobby.rs`. `PlayerNameChangeRequest` and transport-level `Disconnect` variants were removed from the message types.

---

## Identity Negotiation Flow

```
Client          ConnectionHandler    ConnectionManager       LobbyManager

  │── TCP connect + WS handshake ──►│                              │
  │                                 │── ClientEvent::Connected ──►│
  │                                 │                              │ address → negotiating
  │── {"Identity": null} ──────────►│                              │
  │                                 │── ClientEvent::Message ─────►│
  │                                 │                              │ generate unique name
  │                                 │                              │ negotiating → players (PreLobby)
  │                                 │◄── LobbyOutboundMessage ─────│
  │◄── {"PlayerIdentity":"ab3Xz"} ──│                              │
```

Name uniqueness is guaranteed by checking `names_in_use` before assigning. On collision (36^5 ≈ 60M combinations makes this extremely rare), generation retries until a unique name is found.

---

## Random ID Generation

Used for both Player_Names and Lobby_Ids:

```rust
fn random_alphanum_5() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..5)
        .map(|_| {
            let idx = rng.random_range(0..36usize);
            if idx < 10 { (b'0' + idx as u8) as char }
            else        { (b'a' + (idx - 10) as u8) as char }
        })
        .collect()
}
```

---

## Error Handling

### Connection Layer

| Scenario | Handler | Outcome |
|---|---|---|
| WebSocket handshake failure | TCP listener | Log ERROR, continue accepting |
| Unreadable frame | `ConnectionHandler` | Log ERROR, continue select loop |
| Non-text frame (binary, ping, pong) | `ConnectionHandler` | Silently ignore |
| `event_tx.send` fails (manager gone) | `ConnectionHandler` | Exit task |
| `msg_rx.recv` returns `None` | `ConnectionHandler` | Exit task cleanly |
| `Ctrl+C` received | `run()` | Send `Shutdown` to manager and listener |

### Lobby Layer

| Scenario | Response |
|---|---|
| Non-JSON / unparseable text frame | `Error("invalid message")` sent by `ConnectionManager` directly |
| `Identity` when already identified | `Error("already identified")` |
| Non-`Identity` during negotiation | `Error("must send Identity first")` |
| `CreateLobby` / `JoinLobby` when not in Pre_Lobby | `Error("not in pre-lobby state")` |
| `JoinLobby` with unknown lobby_id | `Error("lobby not found")` |
| `JoinLobby` when lobby is full | `Error("lobby is full")` |
| `LeaveLobby` when not in a lobby | `Error("not in a lobby")` |
| `LeaveLobby` during active game | `Error("cannot leave during game")` |
| `StartGame` from non-leader | `Error("only the leader can start the game")` |
| `StartGame` with fewer than 2 players | `Error("need at least 2 players to start")` |
| `Hook` when not player's turn | `HookError(NotYourTurn)` |
| `Hook` targeting unknown player | `HookError(UnknownPlayer(name))` |
| `Hook` targeting self | `HookError(CannotTargetYourself)` |
| `Hook` for rank not in hand | `HookError(YouDoNotHaveRank(rank))` |
| Player disconnects during game | Game ended; `GameResult` (disconnection) to survivors; survivors → Pre_Lobby |
| Player disconnects in lobby | LeaveLobby rules applied; `LobbyUpdated` to remaining |
| Player disconnects in Pre_Lobby | Removed from `players` and `names_in_use` |
| Client disconnects during negotiation | Removed from `negotiating`; no player state |
| `outbound_tx.send` fails | Log WARN; continue (CM shutdown will cascade) |

---

## Correctness Properties

These properties are verified by property-based tests using `proptest`. All can be exercised with in-memory mpsc channels — no TCP required.

### Connection Layer Properties

**P1: Connection registration**
For any client that successfully completes the WebSocket handshake, the `ConnectionManager`'s client registry contains an entry for that client's socket address.
*Validates: Requirements 1.2, 1.3*

**P2: Echo round-trip** *(connection layer baseline, superseded by lobby layer in production)*
For any non-empty text string sent by a connected client, the `ConnectionManager` routes a `ServerMessage::Text` containing the exact same string back to that client.
*Validates: Requirements (connection layer message routing)*

**P3: Echo isolation**
For any set of two or more connected clients, when one client sends a text message, only that client receives the echo.
*Validates: Requirements 4.2*

**P4: Disconnection removes client**
For any connected client, after disconnection (clean or force), the `ConnectionManager`'s registry no longer contains an entry for that client's address.
*Validates: Requirements 3.1, 3.2*

**P5: Disconnect does not affect remaining clients**
For any set of two or more connected clients, after one disconnects, the remaining clients still send and receive correctly.
*Validates: Requirements 3.4*

### Lobby Layer Properties

**P1: Identity uniqueness**
For any sequence of N `Identity` requests from distinct addresses, all assigned Player_Names are unique.
`∀ a₁…aₙ (distinct): |{name(a₁), …, name(aₙ)}| = n`
*Validates: Requirement 7.6*

**P2: Pre_Lobby → In_Lobby transition on CreateLobby**
For any player in Pre_Lobby, after `CreateLobby`, the player's phase is `InLobby` and a `LobbyJoined` message is delivered.
*Validates: Requirements 8.1–8.4*

**P3: Lobby membership invariants**
For any lobby at any point in time: no address appears more than once in `lobby.players`; `players[0]` is always a member; `players.len() ≤ max_players`.
*Validates: Requirements 8.2, 9.1, 9.5, 10.3*

**P4: Game session isolation**
For any two lobbies both in-game, a `Hook` processed in one session produces no state change in the other.
*Validates: Requirement 12 (isolation between concurrent games)*

**P5: Disconnection cleanup**
For any player in any state, after `ClientDisconnected`: address absent from `negotiating`, `players`, and all `lobby.players`; name absent from `names_in_use`.
*Validates: Requirements 14.1–14.4, 15.1–15.4*

---

## Testing Strategy

### Approach

Both unit tests and property-based tests are used. They are complementary:
- **Unit tests** cover specific scenarios, integration points, and edge cases.
- **Property-based tests** (`proptest`) verify universal properties across randomly generated inputs with a minimum of 100 iterations each.

All tests for `ConnectionManager` and `ConnectionHandler` live in `connection.rs`. All tests for `LobbyManager` live in `lobby.rs`. Neither requires a real TCP stack — channels are constructed directly and driven with `#[tokio::test]`. Integration tests that need a real socket bind to `127.0.0.1:0` (OS-assigned ephemeral port).

Tag format for property tests:
```
// Feature: <feature-name>, Property <N>: <property_text>
```

### Connection Layer Tests

Unit tests:
- Server binds to configured address
- Handshake failure on non-WS TCP connection does not stop the server
- Binary frame produces no `ClientEvent::Message`
- Ping frame produces no `ClientEvent::Message`
- `DisconnectReason::Clean` results in client removal
- `DisconnectReason::ForceClosed` results in client removal

Property tests (tag: `go-fish-game-server`):
- P1: Connection registration
- P2: Echo round-trip
- P3: Echo isolation
- P4: Disconnection removes client
- P5: Disconnect does not affect remaining clients

### Lobby Layer Tests

Unit tests:
- `Identity` assigns name, sends `PlayerIdentity`, transitions to Pre_Lobby
- Non-`Identity` during negotiation sends `Error`, client stays in negotiating set
- Duplicate `Identity` from already-identified player sends `Error`
- `CreateLobby` creates lobby, player transitions to `InLobby`, `LobbyJoined` sent
- `JoinLobby` (valid) adds player, `LobbyJoined` to joiner, `LobbyUpdated` to existing members
- `JoinLobby` with unknown id sends `Error`
- `JoinLobby` on full lobby sends `Error`
- Lobby auto-starts when `max_players` reached
- `LeaveLobby` removes player, `LobbyUpdated` sent, leadership transferred if needed
- `LeaveLobby` during game sends `Error`
- Last player leaves lobby — lobby removed
- `StartGame` from non-leader sends `Error`
- `StartGame` with fewer than 2 players sends `Error`
- `StartGame` (leader, ≥2 players) sends `GameStarted` + `HandState` + `PlayerTurn` + `PlayerIdentity` to all
- `Hook` when not player's turn sends `HookError(NotYourTurn)`
- `Hook` targeting unknown player sends `HookError(UnknownPlayer)`
- `Hook` targeting self sends `HookError(CannotTargetYourself)`
- `Hook` for rank not in hand sends `HookError(YouDoNotHaveRank)`
- Valid `Hook` broadcasts `HookAndResult`, updated `HandState`, and `PlayerTurn` to all
- Game ends normally — `GameResult` sent, all players → Pre_Lobby
- Player disconnects during game — `GameResult` (disconnection) to survivors, survivors → Pre_Lobby
- Disconnect during negotiation — removed from `negotiating`, no player state created
- Disconnect in Pre_Lobby — player removed from registry
- Disconnect in lobby — LeaveLobby rules applied

Property tests (tag: `go-fish-lobby-and-game`):
- P1: Identity uniqueness
- P2: Pre_Lobby → In_Lobby on CreateLobby
- P3: Lobby membership invariants
- P4: Game session isolation
- P5: Disconnection cleanup
