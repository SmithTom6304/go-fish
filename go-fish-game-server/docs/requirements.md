# Go Fish Game Server — Requirements

## Introduction

This document consolidates requirements for the `go-fish-game-server` crate across two feature areas:

1. **Connection Management** — the WebSocket connection layer: accepting clients, routing messages, and graceful shutdown.
2. **Lobby and Game** — identity negotiation, lobby creation/joining, and in-game Go Fish play.

The connection layer is the foundation. The lobby and game layer builds on top of it without replacing it.

---

## Glossary

- **Server**: The `go-fish-game-server` process.
- **Client**: A remote peer with an active WebSocket connection to the Server.
- **Player**: A Client that has successfully completed identity negotiation and has an assigned Player_Name.
- **Player_Name**: A unique randomly generated 5-character alphanumeric string identifier for a Player, scoped to the Server instance.
- **Connection**: An active WebSocket session between the Server and a single Client, identified by the Client's socket address.
- **Connection_Handler**: The per-connection async task responsible for reading and writing WebSocket frames for a single Client.
- **Connection_Manager**: The central component that tracks all active Connections, routes messages between Connection_Handlers and the rest of the system, and forwards parsed client messages to the Lobby_Manager.
- **Lobby_Manager**: The component responsible for managing the Pre_Lobby player registry, creating and tracking Lobbies, managing Lobby membership, and managing Game_Sessions.
- **ClientMessage**: A parsed application-level message received from a Client.
- **ServerMessage**: An application-level message sent by the Server to one or more Clients.
- **ManagerCommand**: A server-level control signal sent to the Connection_Manager or TCP Listener to coordinate lifecycle events such as shutdown.
- **Identity_Negotiation_Phase**: The period between WebSocket connection establishment and successful Player_Name assignment.
- **Pre_Lobby**: The state a Player enters after completing identity negotiation. From here a Player can create or join a Lobby.
- **Lobby**: A named waiting room where Players gather before a game starts, identified by a unique Lobby_Id.
- **Lobby_Id**: A randomly generated 5-character alphanumeric string that uniquely identifies a Lobby.
- **Lobby_Leader**: The Player who created a Lobby, or the next Player in join order if the original leader leaves.
- **Game_Session**: An active Go Fish game running within a Lobby, involving all Players who were in the Lobby when the game started.

---

## Part 1: Connection Management

### Requirement 1: Continuous Connection Acceptance

**User Story:** As a server operator, I want the server to continuously accept new client connections, so that clients can join at any time without restarting the server.

#### Acceptance Criteria

1. THE Server SHALL bind to a configurable TCP address and port on startup.
2. WHEN a TCP connection is received, THE Server SHALL perform the WebSocket handshake and register the Connection.
3. WHILE the Server is running, THE Server SHALL accept new Connections without interrupting existing Connections.
4. IF the WebSocket handshake fails for an incoming TCP connection, THEN THE Server SHALL log the error and continue accepting new connections.

### Requirement 2: Per-Client Message Handling

**User Story:** As a client, I want my messages to be received and processed by the server, so that the server can act on what I send.

#### Acceptance Criteria

1. WHEN a Client sends a text message, THE Connection_Handler SHALL receive the message and forward it to the Connection_Manager.
2. WHEN a Client sends a non-text WebSocket frame (e.g. binary, ping), THE Connection_Handler SHALL ignore the frame and continue processing subsequent messages.
3. IF a message cannot be read from the WebSocket stream, THEN THE Connection_Handler SHALL log the error and continue processing subsequent messages.

### Requirement 3: Graceful Client Disconnection

**User Story:** As a server operator, I want the server to handle clients leaving cleanly, so that resources are not leaked and other clients are unaffected.

#### Acceptance Criteria

1. WHEN a Client sends a WebSocket Close frame, THE Connection_Handler SHALL complete the closing handshake and remove the Connection from the Connection_Manager.
2. WHEN a Client disconnects without sending a Close frame (force disconnect), THE Connection_Handler SHALL detect the stream closure, log the event, and remove the Connection from the Connection_Manager.
3. WHEN a Connection is removed, THE Server SHALL release all resources associated with that Connection.
4. WHEN one Client disconnects, THE Server SHALL continue serving all remaining Connections without interruption.

### Requirement 4: Concurrent Client Support

**User Story:** As a server operator, I want the server to handle multiple clients simultaneously, so that more than one client can be connected at the same time.

#### Acceptance Criteria

1. THE Server SHALL support multiple simultaneous Connections.
2. WHILE multiple Clients are connected, THE Server SHALL process messages from each Client independently.
3. WHILE multiple Clients are connected, a slow or unresponsive Client SHALL not block message processing for other Clients.

### Requirement 5: Structured Logging

**User Story:** As a developer, I want the server to emit structured logs for key connection lifecycle events, so that I can observe and debug server behaviour.

#### Acceptance Criteria

1. WHEN a new Connection is established, THE Server SHALL emit a log entry at INFO level containing the Client's socket address.
2. WHEN a Connection is removed, THE Server SHALL emit a log entry at INFO level containing the Client's socket address and the reason for removal.
3. WHEN a ClientMessage is received, THE Server SHALL emit a log entry at DEBUG level containing the Client's socket address and the message content.
4. WHEN an error occurs during message processing, THE Server SHALL emit a log entry at ERROR level containing the Client's socket address and a description of the error.

### Requirement 6: Graceful Server Shutdown

**User Story:** As a server operator, I want the server to shut down cleanly when I send a termination signal, so that connected clients are notified and resources are released without abrupt disconnection.

#### Acceptance Criteria

1. WHEN the server process receives a `Ctrl+C` (SIGINT) signal, THE Server SHALL initiate a graceful shutdown.
2. WHEN a graceful shutdown is initiated, THE Connection_Manager SHALL send a WebSocket Close frame to all connected Clients before exiting.
3. WHEN a graceful shutdown is initiated, THE TCP Listener SHALL stop accepting new Connections.
4. WHEN a graceful shutdown is initiated, THE Server SHALL log the shutdown event at INFO level.

---

## Part 2: Lobby and Game

### Requirement 7: Identity Negotiation

**User Story:** As a client, I want to be assigned a name when I connect, so that I have a recognisable identity in the game.

#### Acceptance Criteria

1. WHEN a Client establishes a WebSocket connection, THE Server SHALL place the Client in the Identity_Negotiation_Phase before allowing any other action.
2. WHEN a Client in the Identity_Negotiation_Phase sends an `Identity` message, THE Server SHALL assign a randomly generated 5-character alphanumeric string as the Player_Name.
3. WHEN a Player_Name is assigned, THE Server SHALL send a `PlayerIdentity` message containing the assigned Player_Name to the Client.
4. WHEN a Player_Name is assigned, THE Server SHALL transition the Client to the Pre_Lobby state.
5. WHILE a Client is in the Identity_Negotiation_Phase, THE Server SHALL reject any message that is not an `Identity` message and send an appropriate error response.
6. THE Server SHALL guarantee that no two Players share the same Player_Name at any point in time.

### Requirement 8: Lobby Creation

**User Story:** As a player, I want to create a new lobby, so that I can invite others to play.

#### Acceptance Criteria

1. WHEN a Player in the Pre_Lobby state sends a `CreateLobby` message, THE Lobby_Manager SHALL create a new Lobby with a randomly generated Lobby_Id.
2. THE Lobby_Manager SHALL assign the creating Player as the Lobby_Leader.
3. THE Lobby_Manager SHALL automatically add the creating Player to the new Lobby and transition the Player out of the Pre_Lobby state.
4. WHEN a Lobby is created, THE Lobby_Manager SHALL send a `LobbyJoined` message to the creating Player containing the Lobby_Id, Lobby_Leader name, current player list, and maximum player count.
5. THE Lobby_Id SHALL be a randomly generated 5-character alphanumeric string that is unique among all currently open Lobbies.
6. THE Server configuration SHALL specify a `lobby_max_players` value, and that value SHALL be at least 2.
7. THE Lobby_Manager SHALL use the `lobby_max_players` value from the Server configuration as the maximum player count for every newly created Lobby.

### Requirement 9: Joining a Lobby

**User Story:** As a player, I want to join an existing lobby by its ID, so that I can play with others.

#### Acceptance Criteria

1. WHEN a Player in the Pre_Lobby state sends a `JoinLobby` message with a Lobby_Id, THE Lobby_Manager SHALL add the Player to that Lobby if it exists and is not full, and transition the Player out of the Pre_Lobby state.
2. WHEN a Player joins a Lobby, THE Lobby_Manager SHALL send a `LobbyJoined` message to the joining Player containing the Lobby_Id, Lobby_Leader name, current player list, and maximum player count.
3. WHEN a Player joins a Lobby, THE Lobby_Manager SHALL send a `LobbyUpdated` message to all other Players already in the Lobby, reflecting the new player list.
4. IF a Player sends a `JoinLobby` message with a Lobby_Id that does not exist, THEN THE Lobby_Manager SHALL send an appropriate error response to that Player.
5. IF a Player sends a `JoinLobby` message for a Lobby that is already at its maximum player count, THEN THE Lobby_Manager SHALL send an appropriate error response to that Player.
6. WHEN a Lobby reaches its maximum player count, THE Lobby_Manager SHALL automatically start the Game_Session for that Lobby.

### Requirement 10: Leaving a Lobby

**User Story:** As a player, I want to leave a lobby and return to the main menu, so that I can change my plans before a game starts.

#### Acceptance Criteria

1. WHEN a Player in a Lobby sends a `LeaveLobby` message, THE Lobby_Manager SHALL remove the Player from the Lobby and transition the Player back to the Pre_Lobby state.
2. WHEN a Player leaves a Lobby, THE Lobby_Manager SHALL send a `LobbyUpdated` message to all remaining Players in the Lobby, reflecting the updated player list.
3. WHEN the Lobby_Leader leaves a Lobby that still has other Players, THE Lobby_Manager SHALL assign the Lobby_Leader role to the next Player in join order.
4. WHEN the Lobby_Leader changes, THE Lobby_Manager SHALL send a `LobbyUpdated` message to all remaining Players in the Lobby indicating the new Lobby_Leader.
5. WHEN the last Player leaves a Lobby, THE Lobby_Manager SHALL close and remove the Lobby.
6. WHILE a Game_Session is in progress, THE Server SHALL reject `LeaveLobby` messages.

### Requirement 11: Starting a Game

**User Story:** As a lobby leader, I want to start the game when I'm ready, so that play can begin.

#### Acceptance Criteria

1. WHEN the Lobby_Leader sends a `StartGame` message and the Lobby has at least 2 Players, THE Lobby_Manager SHALL start a Game_Session for that Lobby.
2. IF a non-leader Player sends a `StartGame` message, THEN THE Lobby_Manager SHALL send an appropriate error response to that Player.
3. IF the Lobby_Leader sends a `StartGame` message and the Lobby has fewer than 2 Players, THEN THE Lobby_Manager SHALL send an appropriate error response.
4. WHEN a Game_Session starts, THE Lobby_Manager SHALL send a `GameStarted` message to all Players in the Lobby.
5. WHEN a Game_Session starts, THE Lobby_Manager SHALL send each Player their initial `HandState` (hand and completed books).
6. WHEN a Game_Session starts, THE Lobby_Manager SHALL send each Player a `PlayerTurn` message indicating whose turn it is.
7. WHEN a Game_Session starts, THE Lobby_Manager SHALL send each Player a `PlayerIdentity` message confirming their Player_Name within the game context.

### Requirement 12: In-Game Play

**User Story:** As a player in a game, I want to take turns hooking other players for cards, so that I can complete books and win.

#### Acceptance Criteria

1. WHEN it is a Player's turn, THE Lobby_Manager SHALL accept a `Hook` message from that Player specifying a target Player_Name and a card rank.
2. WHEN a Player sends a `Hook` message and it is not their turn, THE Lobby_Manager SHALL send a `HookError(NotYourTurn)` response to that Player.
3. WHEN a Player sends a `Hook` message targeting a Player_Name that does not exist in the Game_Session, THE Lobby_Manager SHALL send a `HookError(UnknownPlayer)` response.
4. WHEN a Player sends a `Hook` message targeting themselves, THE Lobby_Manager SHALL send a `HookError(CannotTargetYourself)` response.
5. WHEN a Player sends a `Hook` message for a rank they do not hold in their hand, THE Lobby_Manager SHALL send a `HookError(YouDoNotHaveRank)` response.
6. WHEN a valid `Hook` is processed, THE Lobby_Manager SHALL broadcast a `HookAndResult` message to all Players in the Game_Session.
7. WHEN a valid `Hook` is processed, THE Lobby_Manager SHALL send an updated `HandState` to each Player in the Game_Session.
8. WHEN a valid `Hook` is processed, THE Lobby_Manager SHALL send a `PlayerTurn` message to each Player in the Game_Session indicating whose turn it is next.

### Requirement 13: Game Completion

**User Story:** As a player, I want to know when the game ends and who won, so that the result is clear.

#### Acceptance Criteria

1. WHEN a Game_Session ends, THE Lobby_Manager SHALL send a `GameResult` message to all Players in the Game_Session containing the winners and losers by Player_Name.
2. WHEN a Game_Session ends normally, THE Lobby_Manager SHALL transition all Players back to the Pre_Lobby state.

### Requirement 14: Player Disconnection During a Game

**User Story:** As a server operator, I want the server to handle a player disconnecting mid-game gracefully, so that other players are not left in a broken state.

#### Acceptance Criteria

1. WHEN a Player disconnects during a Game_Session, THE Lobby_Manager SHALL end the Game_Session immediately.
2. WHEN a Game_Session ends due to a Player disconnecting, THE Lobby_Manager SHALL send a `GameResult` message to all remaining connected Players indicating the game ended due to disconnection.
3. WHEN a Game_Session ends due to a Player disconnecting, THE Lobby_Manager SHALL transition all remaining connected Players back to the Pre_Lobby state.
4. WHEN a Player disconnects during a Game_Session, THE Server SHALL NOT transition the disconnected Player to any state (the connection is gone).

### Requirement 15: Player Disconnection Outside a Game

**User Story:** As a server operator, I want the server to clean up player state when a player disconnects outside of a game, so that resources are not leaked.

#### Acceptance Criteria

1. WHEN a Player disconnects, THE Lobby_Manager SHALL remove the Player's identity and release all associated resources.
2. WHEN a Player in a Lobby disconnects, THE Lobby_Manager SHALL remove the Player from the Lobby and apply the same rules as if the Player had sent a `LeaveLobby` message.
3. WHEN a Client in the Identity_Negotiation_Phase disconnects, THE Server SHALL release all associated resources without creating any Player state.
4. WHEN a Player in the Pre_Lobby state disconnects, THE Lobby_Manager SHALL remove the Player from the Pre_Lobby registry and release all associated resources.
