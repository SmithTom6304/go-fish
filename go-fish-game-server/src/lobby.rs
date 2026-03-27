use crate::connection::{LobbyEvent, LobbyOutboundMessage};
use go_fish_web::LobbyLeftReason;
use go_fish_web::{ClientMessage, ServerMessage};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::{debug, info};

// ── Client phase ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ClientPhase {
    IdentityNegotiation,
    PreLobby,
    InLobby { lobby_id: String },
    InGame { lobby_id: String },
}

// ── Player record ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct PlayerRecord {
    pub name: String,
    pub address: SocketAddr,
    pub phase: ClientPhase,
}

// ── Game session ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct GameSession {
    pub game: go_fish::Game,
    pub id_to_name: HashMap<go_fish::PlayerId, String>,
    pub name_to_id: HashMap<String, go_fish::PlayerId>,
    pub name_to_addr: HashMap<String, SocketAddr>,
}

// ── Lobby ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LobbyState {
    Waiting,
    InGame(GameSession),
}

#[derive(Debug)]
pub struct Lobby {
    pub lobby_id: String,
    /// Players in join order; players[0] is always the leader.
    pub players: Vec<SocketAddr>,
    pub max_players: usize,
    pub state: LobbyState,
}

// ── LobbyCommand ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LobbyCommand {
    Shutdown,
}

// ── LobbyManager ─────────────────────────────────────────────────────────────

pub struct LobbyManager {
    negotiating: HashSet<SocketAddr>,
    players: HashMap<SocketAddr, PlayerRecord>,
    names_in_use: HashSet<String>,
    lobbies: HashMap<String, Lobby>,
    lobby_max_players: usize,
    event_rx: mpsc::Receiver<LobbyEvent>,
    outbound_tx: mpsc::Sender<LobbyOutboundMessage>,
    command_rx: mpsc::Receiver<LobbyCommand>,
}

impl LobbyManager {
    pub fn new(
        event_rx: mpsc::Receiver<LobbyEvent>,
        outbound_tx: mpsc::Sender<LobbyOutboundMessage>,
        command_rx: mpsc::Receiver<LobbyCommand>,
        lobby_max_players: usize,
    ) -> Self {
        LobbyManager {
            negotiating: HashSet::new(),
            players: HashMap::new(),
            names_in_use: HashSet::new(),
            lobbies: HashMap::new(),
            lobby_max_players,
            event_rx,
            outbound_tx,
            command_rx,
        }
    }

    pub fn event_tx_channel() -> (mpsc::Sender<LobbyEvent>, mpsc::Receiver<LobbyEvent>) {
        mpsc::channel(64)
    }

    pub fn outbound_channel() -> (mpsc::Sender<LobbyOutboundMessage>, mpsc::Receiver<LobbyOutboundMessage>) {
        mpsc::channel(64)
    }

    pub fn command_channel() -> (mpsc::Sender<LobbyCommand>, mpsc::Receiver<LobbyCommand>) {
        mpsc::channel(8)
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(LobbyCommand::Shutdown) | None => break,
                    }
                }
                event = self.event_rx.recv() => {
                    match event {
                        None => break,
                        Some(e) => self.handle_event(e).await,
                    }
                }
            }
        }
    }

    async fn handle_event(&mut self, event: LobbyEvent) {
        match event {
            LobbyEvent::ClientConnected { address } => {
                self.negotiating.insert(address);
                debug!(%address, "client entered identity negotiation phase");
            }

            LobbyEvent::ClientMessage { address, message } => {
                // Reject non-Identity messages during negotiation
                if self.negotiating.contains(&address) {
                    if !matches!(message, ClientMessage::Identity) {
                        self.send(address, ServerMessage::Error("must send Identity first".to_string())).await;
                        return;
                    }
                    // Handle Identity for negotiating clients
                    let mut name = random_alphanum_5();
                    while self.names_in_use.contains(&name) {
                        name = random_alphanum_5();
                    }
                    self.negotiating.remove(&address);
                    self.players.insert(address, PlayerRecord {
                        name: name.clone(),
                        address,
                        phase: ClientPhase::PreLobby,
                    });
                    self.names_in_use.insert(name.clone());
                    self.send(address, ServerMessage::PlayerIdentity(name.clone())).await;
                    info!(%address, name = %name, "player identity assigned");
                    return;
                }

                // Reject duplicate Identity from already-identified players
                if self.players.contains_key(&address) {
                    if matches!(message, ClientMessage::Identity) {
                        self.send(address, ServerMessage::Error("already identified".to_string())).await;
                        return;
                    }
                }

                // Route to player message handler
                self.handle_player_message(address, message).await;
            }

            LobbyEvent::ClientDisconnected { address, .. } => {
                if self.negotiating.remove(&address) {
                    // Was in negotiation — no player state to clean up
                    return;
                }
                if let Some(record) = self.players.get(&address) {
                    let phase = record.phase.clone();
                    match phase {
                        ClientPhase::InLobby { lobby_id } => {
                            let msgs = self.remove_player_from_lobby(address, &lobby_id);
                            for (addr, msg) in msgs {
                                self.send(addr, msg).await;
                            }
                        }
                        ClientPhase::InGame { lobby_id } => {
                            // End the game session due to disconnection
                            // Do NOT send any message to the disconnecting address
                            // Remove the disconnecting player from the session's name_to_addr so they don't receive GameResult
                            if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
                                if let LobbyState::InGame(session) = &mut lobby.state {
                                    // Remove disconnecting player from session maps so they don't receive messages
                                    let name = self.players.get(&address).map(|r| r.name.clone());
                                    if let Some(name) = name {
                                        session.name_to_addr.remove(&name);
                                    }
                                }
                            }
                            self.end_game_session(lobby_id, true).await;
                        }
                        _ => {}
                    }
                    let name = self.players[&address].name.clone();
                    self.players.remove(&address);
                    self.names_in_use.remove(&name);
                }
            }
        }
    }

    async fn handle_player_message(&mut self, address: SocketAddr, message: ClientMessage) {
        match message {
            ClientMessage::CreateLobby => {
                // Must be in PreLobby phase
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                if !matches!(phase, Some(ClientPhase::PreLobby)) {
                    self.send(address, ServerMessage::Error("not in pre-lobby state".to_string())).await;
                    return;
                }

                // Generate unique lobby_id
                let mut lobby_id = random_alphanum_5();
                while self.lobbies.contains_key(&lobby_id) {
                    lobby_id = random_alphanum_5();
                }

                let name = self.players[&address].name.clone();

                // Create lobby
                let lobby = Lobby {
                    lobby_id: lobby_id.clone(),
                    players: vec![address],
                    max_players: self.lobby_max_players,
                    state: LobbyState::Waiting,
                };
                self.lobbies.insert(lobby_id.clone(), lobby);

                // Update player phase
                if let Some(record) = self.players.get_mut(&address) {
                    record.phase = ClientPhase::InLobby { lobby_id: lobby_id.clone() };
                }

                info!(lobby_id = %lobby_id, leader = %name, "lobby created");

                self.send(address, ServerMessage::LobbyJoined {
                    lobby_id,
                    leader: name.clone(),
                    players: vec![name],
                    max_players: self.lobby_max_players,
                }).await;
            }

            ClientMessage::JoinLobby(lobby_id) => {
                // Must be in PreLobby phase
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                if !matches!(phase, Some(ClientPhase::PreLobby)) {
                    self.send(address, ServerMessage::Error("not in pre-lobby state".to_string())).await;
                    return;
                }

                // Lobby must exist
                if !self.lobbies.contains_key(&lobby_id) {
                    self.send(address, ServerMessage::Error("lobby not found".to_string())).await;
                    return;
                }

                {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    // Lobby must not be in-game
                    if matches!(lobby.state, LobbyState::InGame(_)) {
                       self.send(address, ServerMessage::Error("lobby is in game".to_string())).await;
                        return;
                    }
                    // Lobby must not be full
                    if lobby.players.len() >= lobby.max_players {
                        self.send(address, ServerMessage::Error("lobby is full".to_string())).await;
                        return;
                    }
                }

                let joining_name = self.players[&address].name.clone();

                // Add player to lobby
                self.lobbies.get_mut(&lobby_id).unwrap().players.push(address);

                // Update player phase
                if let Some(record) = self.players.get_mut(&address) {
                    record.phase = ClientPhase::InLobby { lobby_id: lobby_id.clone() };
                }

                // Build player names list and leader name
                let (leader_name, players_names, other_players, max_players) = {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    let leader_addr = lobby.players[0];
                    let leader_name = self.players[&leader_addr].name.clone();
                    let players_names: Vec<String> = lobby.players.iter()
                        .map(|a| self.players[a].name.clone())
                        .collect();
                    let other_players: Vec<SocketAddr> = lobby.players.iter()
                        .copied()
                        .filter(|&a| a != address)
                        .collect();
                    (leader_name, players_names, other_players, lobby.max_players)
                };

                info!(lobby_id = %lobby_id, player = %joining_name, "player joined lobby");

                // Send LobbyJoined to joining player
                self.send(address, ServerMessage::LobbyJoined {
                    lobby_id: lobby_id.clone(),
                    leader: leader_name.clone(),
                    players: players_names.clone(),
                    max_players,
                }).await;

                // Send LobbyUpdated to all other players
                for other_addr in other_players {
                    self.send(other_addr, ServerMessage::LobbyUpdated {
                        leader: leader_name.clone(),
                        players: players_names.clone(),
                    }).await;
                }

                // Auto-start if lobby is now full
                let current_len = self.lobbies.get(&lobby_id).map(|l| l.players.len()).unwrap_or(0);
                if current_len >= max_players {
                    self.start_game_session(lobby_id).await;
                }
            }

            ClientMessage::LeaveLobby => {
                // Must be in InLobby or InGame phase
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InLobby { lobby_id }) => lobby_id,
                    Some(ClientPhase::InGame { lobby_id: _ }) => {
                        self.send(address, ServerMessage::Error("cannot leave during game".to_string())).await;
                        return;
                    }
                    _ => {
                        self.send(address, ServerMessage::Error("not in a lobby".to_string())).await;
                        return;
                    }
                };

                // Cannot leave during a game (check lobby state as well)
                if let Some(lobby) = self.lobbies.get(&lobby_id) {
                    if matches!(lobby.state, LobbyState::InGame(_)) {
                        self.send(address, ServerMessage::Error("cannot leave during game".to_string())).await;
                        return;
                    }
                }

                let player_name = self.players.get(&address).map(|r| r.name.clone()).unwrap_or_default();

                let msgs = self.remove_player_from_lobby(address, &lobby_id);

                for (addr, msg) in msgs {
                    self.send(addr, msg).await;
                }

                // Update player phase back to PreLobby
                if let Some(record) = self.players.get_mut(&address) {
                    record.phase = ClientPhase::PreLobby;
                }

                // Send LeftLobby to client
                self.send(address, ServerMessage::LobbyLeft(LobbyLeftReason::RequestedByPlayer)).await;

                info!(lobby_id = %lobby_id, player = %player_name, "player left lobby");
            }

            ClientMessage::StartGame => {
                // Must be in InLobby phase
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InLobby { lobby_id }) => lobby_id,
                    _ => {
                        self.send(address, ServerMessage::Error("not in a lobby".to_string())).await;
                        return;
                    }
                };

                // Only the leader (players[0]) can start the game
                let is_leader = self.lobbies.get(&lobby_id)
                    .map(|l| l.players.first() == Some(&address))
                    .unwrap_or(false);
                if !is_leader {
                    self.send(address, ServerMessage::Error("only the leader can start the game".to_string())).await;
                    return;
                }

                // Need at least 2 players
                let player_count = self.lobbies.get(&lobby_id)
                    .map(|l| l.players.len())
                    .unwrap_or(0);
                if player_count < 2 {
                    self.send(address, ServerMessage::Error("need at least 2 players to start".to_string())).await;
                    return;
                }

                self.start_game_session(lobby_id).await;
            }

            ClientMessage::Hook(hook_request) => {
                // Must be in InGame phase
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InGame { lobby_id }) => lobby_id,
                    _ => {
                        self.send(address, ServerMessage::Error("not in a game".to_string())).await;
                        return;
                    }
                };

                let sender_name = self.players[&address].name.clone();

                // Validate and collect data from session (immutable borrow)
                enum HookValidation {
                    Valid { target_player_id: go_fish::PlayerId },
                    Invalid(ServerMessage),
                    UnknownTarget(String),
                }

                let validation = {
                    let lobby = match self.lobbies.get(&lobby_id) {
                        Some(l) => l,
                        None => {
                            self.send(address, ServerMessage::Error("lobby not found".to_string())).await;
                            return;
                        }
                    };
                    let session = match &lobby.state {
                        LobbyState::InGame(s) => s,
                        _ => {
                            self.send(address, ServerMessage::Error("not in a game".to_string())).await;
                            return;
                        }
                    };

                    let sender_player_id = session.name_to_id[&sender_name];

                    // Validation 1: check it's the sender's turn
                    let current_player = session.game.get_current_player();
                    if current_player.map(|p| p.id) != Some(sender_player_id) {
                        HookValidation::Invalid(ServerMessage::HookError(go_fish_web::HookError::NotYourTurn))
                    } else {
                        // Validation 2: check target name exists
                        match session.name_to_id.get(&hook_request.target_name) {
                            None => HookValidation::UnknownTarget(hook_request.target_name.clone()),
                            Some(&target_player_id) => {
                                // Validation 3: check target is not self
                                if target_player_id == sender_player_id {
                                    HookValidation::Invalid(ServerMessage::HookError(go_fish_web::HookError::CannotTargetYourself))
                                } else {
                                    // Validation 4: check sender holds the rank
                                    let current_player = session.game.get_current_player().unwrap();
                                    let has_rank = current_player.hand.books.iter().any(|b| b.rank == hook_request.rank);
                                    if !has_rank {
                                        HookValidation::Invalid(ServerMessage::HookError(go_fish_web::HookError::YouDoNotHaveRank(hook_request.rank)))
                                    } else {
                                        HookValidation::Valid { target_player_id }
                                    }
                                }
                            }
                        }
                    }
                };

                let target_player_id = match validation {
                    HookValidation::Invalid(err_msg) => {
                        self.send(address, err_msg).await;
                        return;
                    }
                    HookValidation::UnknownTarget(name) => {
                        self.send(address, ServerMessage::HookError(go_fish_web::HookError::UnknownPlayer(name))).await;
                        return;
                    }
                    HookValidation::Valid { target_player_id } => target_player_id,
                };

                // Collect player addresses and target name before mutable borrow
                let (player_addrs_names, target_name_str): (Vec<(SocketAddr, String)>, String) = {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    let session = match &lobby.state {
                        LobbyState::InGame(s) => s,
                        _ => return,
                    };
                    let addrs_names: Vec<(SocketAddr, String)> = session.name_to_addr.iter()
                        .map(|(name, &addr)| (addr, name.clone()))
                        .collect();
                    let target_name = session.id_to_name[&target_player_id].clone();
                    (addrs_names, target_name)
                };

                // Process the hook (mutable borrow)
                let result = {
                    let lobby = self.lobbies.get_mut(&lobby_id).unwrap();
                    let session = match &mut lobby.state {
                        LobbyState::InGame(s) => s,
                        _ => return,
                    };
                    session.game.take_turn(go_fish::Hook { target: target_player_id, rank: hook_request.rank })
                };

                let result = match result {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("take_turn error: {:?}", e);
                        return;
                    }
                };

                // Build HookAndResult message
                let hook_and_result_msg = ServerMessage::HookAndResult(go_fish_web::HookAndResult {
                    hook_request: go_fish_web::FullHookRequest {
                        fisher_name: sender_name.clone(),
                        target_name: target_name_str,
                        rank: hook_request.rank,
                    },
                    hook_result: result.clone(),
                });

                // Collect updated game state
                let (game_players, inactive_players, current_player_name, is_finished) = {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    let session = match &lobby.state {
                        LobbyState::InGame(s) => s,
                        _ => return,
                    };
                    let game_players = session.game.players.clone();
                    let inactive_players = session.game.inactive_players.clone();
                    let current_player_id = session.game.get_current_player().map(|p| p.id);
                    let current_player_name = current_player_id
                        .and_then(|id| session.id_to_name.get(&id))
                        .cloned()
                        .unwrap_or_default();
                    (game_players, inactive_players, current_player_name, session.game.is_finished)
                };

                // Send HookAndResult to all players
                for (addr, _) in &player_addrs_names {
                    self.send(*addr, hook_and_result_msg.clone()).await;
                }

                // Send updated HandState to each player
                for (addr, name) in &player_addrs_names {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    let session = match &lobby.state {
                        LobbyState::InGame(s) => s,
                        _ => continue,
                    };
                    let player_id = session.name_to_id[name];
                    if let Some(gf_player) = game_players.iter().find(|p| p.id == player_id) {
                        self.send(*addr, ServerMessage::HandState(go_fish_web::HandState {
                            hand: gf_player.hand.clone(),
                            completed_books: gf_player.completed_books.clone(),
                        })).await;
                    } else if let Some(inactive) = inactive_players.iter().find(|p| p.id == player_id) {
                        self.send(*addr, ServerMessage::HandState(go_fish_web::HandState {
                            hand: go_fish::Hand::empty(),
                            completed_books: inactive.completed_books.clone(),
                        })).await;
                    }
                }

                // Send PlayerTurn to each player
                for (addr, name) in &player_addrs_names {
                    let turn_msg = if name == &current_player_name {
                        go_fish_web::PlayerTurnValue::YourTurn
                    } else {
                        go_fish_web::PlayerTurnValue::OtherTurn(current_player_name.clone())
                    };
                    self.send(*addr, ServerMessage::PlayerTurn(turn_msg)).await;
                }

                // If game is finished, end the session
                if is_finished {
                    self.end_game_session(lobby_id, false).await;
                }
            }

            ClientMessage::Identity => {
                // Already handled above (duplicate identity)
            }
        }
    }

    async fn end_game_session(&mut self, lobby_id: String, disconnection: bool) {
        // Get the session data we need
        let (player_addrs_names, game_result_msg) = {
            let lobby = match self.lobbies.get(&lobby_id) {
                Some(l) => l,
                None => return,
            };
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };

            let player_addrs_names: Vec<(SocketAddr, String)> = session.name_to_addr.iter()
                .map(|(name, &addr)| (addr, name.clone()))
                .collect();

            let game_result_msg = if disconnection {
                // Game ended due to disconnection — no winners/losers
                ServerMessage::GameResult(go_fish_web::GameResult {
                    winners: vec![],
                    losers: player_addrs_names.iter().map(|(_, name)| name.clone()).collect(),
                })
            } else {
                // Normal game end — use go_fish::Game::get_game_result()
                match session.game.get_game_result() {
                    Some(result) => {
                        let winners = result.winners.iter()
                            .filter_map(|p| session.id_to_name.get(&p.id))
                            .cloned()
                            .collect();
                        let losers = result.losers.iter()
                            .filter_map(|p| session.id_to_name.get(&p.id))
                            .cloned()
                            .collect();
                        ServerMessage::GameResult(go_fish_web::GameResult { winners, losers })
                    }
                    None => ServerMessage::GameResult(go_fish_web::GameResult {
                        winners: vec![],
                        losers: vec![],
                    }),
                }
            };

            (player_addrs_names, game_result_msg)
        };

        // Send GameResult to all connected players
        for (addr, _) in &player_addrs_names {
            self.send(*addr, game_result_msg.clone()).await;
        }

        // Transition all players back to PreLobby
        for (addr, _) in &player_addrs_names {
            if let Some(record) = self.players.get_mut(addr) {
                record.phase = ClientPhase::PreLobby;
            }
        }

        // Reset lobby state to Waiting (keep the lobby around for potential reuse)
        if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
            lobby.state = LobbyState::Waiting;
        }

        info!(lobby_id = %lobby_id, disconnection, "game session ended");
    }

    /// Remove a player from a lobby and return the messages to send to remaining players.
    /// Does NOT update the player's phase — caller is responsible for that.
    /// Does NOT send any message to the leaving player.
    fn remove_player_from_lobby(&mut self, address: SocketAddr, lobby_id: &str) -> Vec<(SocketAddr, ServerMessage)> {
        let lobby = match self.lobbies.get_mut(lobby_id) {
            Some(l) => l,
            None => return vec![],
        };

        // Remove the player from the lobby's player list
        lobby.players.retain(|&a| a != address);

        if lobby.players.is_empty() {
            // Last player left — remove the lobby entirely
            self.lobbies.remove(lobby_id);
            return vec![];
        }

        // Build updated player list and leader name
        let remaining: Vec<SocketAddr> = self.lobbies[lobby_id].players.clone();
        let leader_addr = remaining[0];
        let leader_name = self.players[&leader_addr].name.clone();
        let players_names: Vec<String> = remaining.iter()
            .map(|a| self.players[a].name.clone())
            .collect();

        let msg = ServerMessage::LobbyUpdated {
            leader: leader_name,
            players: players_names,
        };

        remaining.iter().map(|&addr| (addr, msg.clone())).collect()
    }

    async fn start_game_session(&mut self, lobby_id: String) {
        let lobby = match self.lobbies.get(&lobby_id) {
            Some(l) => l,
            None => return,
        };

        // Build GameSession
        let player_addrs: Vec<SocketAddr> = lobby.players.clone();
        let player_count = player_addrs.len() as u8;

        let mut deck = go_fish::Deck::new();
        deck.shuffle();
        let game = go_fish::Game::new(deck, player_count);

        let mut id_to_name: HashMap<go_fish::PlayerId, String> = HashMap::new();
        let mut name_to_id: HashMap<String, go_fish::PlayerId> = HashMap::new();
        let mut name_to_addr: HashMap<String, SocketAddr> = HashMap::new();

        for (i, &addr) in player_addrs.iter().enumerate() {
            let player_id = go_fish::PlayerId(i as u8);
            let name = self.players[&addr].name.clone();
            id_to_name.insert(player_id, name.clone());
            name_to_id.insert(name.clone(), player_id);
            name_to_addr.insert(name, addr);
        }

        let session = GameSession { game, id_to_name, name_to_id, name_to_addr };

        // Update lobby state and player phases
        if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
            lobby.state = LobbyState::InGame(session);
        }
        for &addr in &player_addrs {
            if let Some(record) = self.players.get_mut(&addr) {
                record.phase = ClientPhase::InGame { lobby_id: lobby_id.clone() };
            }
        }

        // Send GameStarted to all players
        for &addr in &player_addrs {
            self.send(addr, ServerMessage::GameStarted).await;
        }

        // Send PlayerIdentity, HandState, PlayerTurn to each player
        let lobby = match self.lobbies.get(&lobby_id) {
            Some(l) => l,
            None => return,
        };
        let session = match &lobby.state {
            LobbyState::InGame(s) => s,
            _ => return,
        };

        let current_player_id = session.game.get_current_player().map(|p| p.id);
        let current_player_name = current_player_id
            .and_then(|id| session.id_to_name.get(&id))
            .cloned()
            .unwrap_or_default();

        // Collect what we need before the loop to avoid borrow issues
        let player_data: Vec<(SocketAddr, String, go_fish::PlayerId)> = player_addrs.iter()
            .map(|&addr| {
                let name = self.players[&addr].name.clone();
                let player_id = session.name_to_id[&name];
                (addr, name, player_id)
            })
            .collect();

        let game_players: Vec<go_fish::Player> = session.game.players.clone();
        let current_name = current_player_name.clone();

        for (addr, name, player_id) in &player_data {
            // Find the go_fish::Player for this player
            let gf_player = game_players.iter().find(|p| p.id == *player_id);

            self.send(*addr, ServerMessage::PlayerIdentity(name.clone())).await;

            if let Some(gf_player) = gf_player {
                self.send(*addr, ServerMessage::HandState(go_fish_web::HandState {
                    hand: gf_player.hand.clone(),
                    completed_books: gf_player.completed_books.clone(),
                })).await;
            }

            let turn_msg = if name == &current_name {
                go_fish_web::PlayerTurnValue::YourTurn
            } else {
                go_fish_web::PlayerTurnValue::OtherTurn(current_name.clone())
            };
            self.send(*addr, ServerMessage::PlayerTurn(turn_msg)).await;
        }

        info!(lobby_id = %lobby_id, player_count = player_addrs.len(), "game session started");
    }

    async fn send(&self, address: SocketAddr, message: ServerMessage) {
        let msg = LobbyOutboundMessage { address, message };
        if self.outbound_tx.send(msg).await.is_err() {
            tracing::warn!(%address, "failed to send outbound message — ConnectionManager gone");
        }
    }
}

/// Generate a random 5-character alphanumeric string.
pub fn random_alphanum_5() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..5)
        .map(|_| {
            let idx = rng.random_range(0..36usize);
            if idx < 10 {
                (b'0' + idx as u8) as char
            } else {
                (b'a' + (idx - 10) as u8) as char
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use go_fish_web::ClientMessage;
    use proptest::prelude::*;
    use std::net::SocketAddr;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    fn make_lobby_manager(max_players: usize) -> (
        mpsc::Sender<LobbyEvent>,
        mpsc::Receiver<LobbyOutboundMessage>,
        mpsc::Sender<LobbyCommand>,
        tokio::task::JoinHandle<()>,
    ) {
        let (event_tx, event_rx) = mpsc::channel::<LobbyEvent>(64);
        let (outbound_tx, outbound_rx) = mpsc::channel::<LobbyOutboundMessage>(64);
        let (cmd_tx, cmd_rx) = mpsc::channel::<LobbyCommand>(8);
        let manager = LobbyManager::new(event_rx, outbound_tx, cmd_rx, max_players);
        let handle = tokio::spawn(manager.run());
        (event_tx, outbound_rx, cmd_tx, handle)
    }

    fn addr(n: u16) -> SocketAddr {
        format!("127.0.0.1:{}", 20000 + n).parse().unwrap()
    }

    // -------------------------------------------------------------------------
    // Test: Identity assigns name and sends PlayerIdentity
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn identity_assigns_name_and_sends_player_identity() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let address = addr(1);

        event_tx.send(LobbyEvent::ClientConnected { address }).await.unwrap();
        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::Identity,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        assert_eq!(msg.address, address);
        if let ServerMessage::PlayerIdentity(name) = msg.message {
            assert_eq!(name.len(), 5);
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric()));
        } else {
            panic!("expected PlayerIdentity, got {:?}", msg.message);
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: non-Identity during negotiation sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn non_identity_during_negotiation_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let address = addr(2);

        event_tx.send(LobbyEvent::ClientConnected { address }).await.unwrap();
        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        assert_eq!(msg.address, address);
        assert!(
            matches!(msg.message, ServerMessage::Error(ref e) if e == "must send Identity first"),
            "expected Error('must send Identity first'), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: duplicate Identity from already-identified player sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn duplicate_identity_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let address = addr(3);

        // First: connect and identify
        event_tx.send(LobbyEvent::ClientConnected { address }).await.unwrap();
        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::Identity,
        }).await.unwrap();

        // Consume the PlayerIdentity response
        let first = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(matches!(first.message, ServerMessage::PlayerIdentity(_)));

        // Second Identity — should get Error
        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::Identity,
        }).await.unwrap();

        let second = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        assert_eq!(second.address, address);
        assert!(
            matches!(second.message, ServerMessage::Error(ref e) if e == "already identified"),
            "expected Error('already identified'), got {:?}", second.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // =========================================================================
    // Property-based tests
    // =========================================================================

    macro_rules! prop_async {
        ($body:expr) => {{
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move { $body })
        }};
    }

    // -------------------------------------------------------------------------
    // Property 1: Identity uniqueness
    // Feature: go-fish-lobby-and-game, Property 1: Identity uniqueness
    // Validates: Requirement 1.6
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_identity_uniqueness(n in 2usize..=10usize) {
            prop_async!({
                let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(16);

                // Generate N distinct socket addresses
                let addresses: Vec<SocketAddr> = (0..n)
                    .map(|i| format!("127.0.0.2:{}", 30000 + i as u16).parse().unwrap())
                    .collect();

                // Send ClientConnected + Identity for each
                for &address in &addresses {
                    event_tx.send(LobbyEvent::ClientConnected { address }).await.unwrap();
                    event_tx.send(LobbyEvent::ClientMessage {
                        address,
                        message: ClientMessage::Identity,
                    }).await.unwrap();
                }

                // Collect all assigned names
                let mut names = Vec::new();
                for _ in 0..n {
                    let msg = timeout(Duration::from_secs(5), outbound_rx.recv())
                        .await
                        .expect("timed out waiting for PlayerIdentity")
                        .expect("channel closed");
                    if let ServerMessage::PlayerIdentity(name) = msg.message {
                        names.push(name);
                    } else {
                        panic!("expected PlayerIdentity, got {:?}", msg.message);
                    }
                }

                // Assert all names are unique
                let unique: std::collections::HashSet<_> = names.iter().collect();
                prop_assert_eq!(unique.len(), n, "names should all be unique: {:?}", names);

                cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
                Ok::<(), TestCaseError>(())
            }).unwrap();
        }
    }

    // =========================================================================
    // Helper: connect + identify a player, returns their assigned name
    // =========================================================================
    async fn connect_and_identify(
        event_tx: &mpsc::Sender<LobbyEvent>,
        outbound_rx: &mut mpsc::Receiver<LobbyOutboundMessage>,
        address: SocketAddr,
    ) -> String {
        event_tx.send(LobbyEvent::ClientConnected { address }).await.unwrap();
        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::Identity,
        }).await.unwrap();
        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out waiting for PlayerIdentity")
            .expect("channel closed");
        if let ServerMessage::PlayerIdentity(name) = msg.message {
            name
        } else {
            panic!("expected PlayerIdentity, got {:?}", msg.message);
        }
    }

    // =========================================================================
    // Unit tests: lobby creation and joining (Task 4.4)
    // =========================================================================

    // -------------------------------------------------------------------------
    // Test: CreateLobby creates lobby and sends LobbyJoined
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn create_lobby_creates_lobby_and_sends_lobby_joined() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let address = addr(10);

        let name = connect_and_identify(&event_tx, &mut outbound_rx, address).await;

        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        assert_eq!(msg.address, address);
        match msg.message {
            ServerMessage::LobbyJoined { lobby_id, leader, players, max_players } => {
                assert_eq!(lobby_id.len(), 5);
                assert_eq!(leader, name);
                assert_eq!(players, vec![name]);
                assert_eq!(max_players, 4);
            }
            other => panic!("expected LobbyJoined, got {:?}", other),
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: JoinLobby (valid) sends LobbyJoined to joiner and LobbyUpdated to existing
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn join_lobby_valid_sends_lobby_joined_and_lobby_updated() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(20);
        let addr_b = addr(21);

        // Player A creates lobby
        let name_a = connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_joined_a = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        let lobby_id = match lobby_joined_a.message {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // Player B joins
        let name_b = connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Collect next two messages (order: LobbyJoined to B, LobbyUpdated to A)
        let mut got_lobby_joined_b = false;
        let mut got_lobby_updated_a = false;
        for _ in 0..2 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
            match (msg.address, &msg.message) {
                (a, ServerMessage::LobbyJoined { lobby_id: lid, leader, players, max_players })
                    if a == addr_b =>
                {
                    assert_eq!(lid, &lobby_id);
                    assert_eq!(leader, &name_a);
                    assert!(players.contains(&name_a));
                    assert!(players.contains(&name_b));
                    assert_eq!(*max_players, 4);
                    got_lobby_joined_b = true;
                }
                (a, ServerMessage::LobbyUpdated { leader, players })
                    if a == addr_a =>
                {
                    assert_eq!(leader, &name_a);
                    assert!(players.contains(&name_a));
                    assert!(players.contains(&name_b));
                    got_lobby_updated_a = true;
                }
                other => panic!("unexpected message: {:?}", other),
            }
        }
        assert!(got_lobby_joined_b, "player B should receive LobbyJoined");
        assert!(got_lobby_updated_a, "player A should receive LobbyUpdated");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: JoinLobby with unknown id sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn join_lobby_unknown_id_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let address = addr(30);

        connect_and_identify(&event_tx, &mut outbound_rx, address).await;

        event_tx.send(LobbyEvent::ClientMessage {
            address,
            message: ClientMessage::JoinLobby("xxxxx".to_string()),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");

        assert!(
            matches!(msg.message, ServerMessage::Error(ref e) if e == "lobby not found"),
            "expected Error('lobby not found'), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: JoinLobby on full lobby sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn join_lobby_full_sends_error() {
        // max_players=2: A creates, B joins (fills it), C tries to join
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(40);
        let addr_b = addr(41);
        let addr_c = addr(42);

        // A creates lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins (fills lobby, triggers auto-start)
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B), LobbyUpdated (A), and all game-start messages (8 total)
        for _ in 0..10 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // C tries to join the full lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_c).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_c,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");

        // The lobby is both full and in-game; either error is acceptable
        assert!(
            matches!(&msg.message, ServerMessage::Error(e) if e == "lobby is full" || e == "lobby is in game"),
            "expected Error about full/in-game lobby, got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: auto-start triggered when lobby full (stub — no panic, LobbyJoined sent)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn auto_start_triggered_when_lobby_full() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(50);
        let addr_b = addr(51);

        // A creates lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins — fills lobby, auto-start triggered
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Collect all messages: LobbyJoined(B), LobbyUpdated(A), and 8 game-start messages
        let mut got_lobby_joined = false;
        for _ in 0..10 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
            if msg.address == addr_b && matches!(msg.message, ServerMessage::LobbyJoined { .. }) {
                got_lobby_joined = true;
            }
        }
        assert!(got_lobby_joined, "player B should receive LobbyJoined");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // =========================================================================
    // Property 2: Pre_Lobby → In_Lobby transition on CreateLobby
    // Feature: go-fish-lobby-and-game, Property 2: Pre_Lobby → In_Lobby transition on CreateLobby
    // Validates: Requirements 2.1–2.4
    // =========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_pre_lobby_to_in_lobby_on_create_lobby(max_players in 2usize..=8usize) {
            prop_async!({
                let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(max_players);
                let address: SocketAddr = "127.0.0.3:40000".parse().unwrap();

                let name = connect_and_identify(&event_tx, &mut outbound_rx, address).await;

                event_tx.send(LobbyEvent::ClientMessage {
                    address,
                    message: ClientMessage::CreateLobby,
                }).await.unwrap();

                let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                    .await
                    .expect("timed out waiting for LobbyJoined")
                    .expect("channel closed");

                prop_assert_eq!(msg.address, address);
                match msg.message {
                    ServerMessage::LobbyJoined { lobby_id, leader, players, max_players: mp } => {
                        prop_assert_eq!(lobby_id.len(), 5, "lobby_id should be 5 chars");
                        prop_assert_eq!(&leader, &name, "leader should be the creating player");
                        prop_assert_eq!(players, vec![name.clone()], "players list should contain only the creator");
                        prop_assert_eq!(mp, max_players, "max_players should match config");
                    }
                    other => {
                        return Err(TestCaseError::fail(format!("expected LobbyJoined, got {:?}", other)));
                    }
                }

                cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
                Ok::<(), TestCaseError>(())
            }).unwrap();
        }
    }

    // =========================================================================
    // Property 3: Lobby membership invariants
    // Feature: go-fish-lobby-and-game, Property 3: Lobby membership invariants
    // Validates: Requirements 2.2, 3.1, 3.5, 4.3
    // =========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_lobby_membership_invariants(n in 2usize..=4usize) {
            prop_async!({
                // Use max_players > n so lobby never auto-starts
                let max_players = n + 2;
                let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(max_players);

                // Connect and identify all players
                let addresses: Vec<SocketAddr> = (0..n)
                    .map(|i| format!("127.0.0.4:{}", 50000 + i as u16).parse().unwrap())
                    .collect();

                let mut names = Vec::new();
                for &address in &addresses {
                    let name = connect_and_identify(&event_tx, &mut outbound_rx, address).await;
                    names.push(name);
                }

                // First player creates lobby
                event_tx.send(LobbyEvent::ClientMessage {
                    address: addresses[0],
                    message: ClientMessage::CreateLobby,
                }).await.unwrap();
                let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
                    .await.expect("timed out").expect("channel closed").message
                {
                    ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
                    other => return Err(TestCaseError::fail(format!("expected LobbyJoined, got {:?}", other))),
                };

                // Remaining players join one by one
                for i in 1..n {
                    event_tx.send(LobbyEvent::ClientMessage {
                        address: addresses[i],
                        message: ClientMessage::JoinLobby(lobby_id.clone()),
                    }).await.unwrap();

                    // Collect messages: LobbyJoined for joiner + LobbyUpdated for each existing player
                    let expected_msgs = i + 1; // 1 LobbyJoined + i LobbyUpdated
                    let mut player_counts_in_updates: Vec<usize> = Vec::new();
                    let mut leaders_seen: Vec<String> = Vec::new();

                    for _ in 0..expected_msgs {
                        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                            .await.expect("timed out").expect("channel closed");
                        match &msg.message {
                            ServerMessage::LobbyJoined { players, leader, max_players: mp, .. } => {
                                // Player count must not exceed max_players
                                prop_assert!(players.len() <= *mp,
                                    "players.len() {} > max_players {}", players.len(), mp);
                                // No duplicates in players list
                                let unique: std::collections::HashSet<_> = players.iter().collect();
                                prop_assert_eq!(unique.len(), players.len(),
                                    "duplicate players in LobbyJoined: {:?}", players);
                                player_counts_in_updates.push(players.len());
                                leaders_seen.push(leader.clone());
                            }
                            ServerMessage::LobbyUpdated { players, leader } => {
                                // Player count must not exceed max_players
                                prop_assert!(players.len() <= max_players,
                                    "players.len() {} > max_players {}", players.len(), max_players);
                                // No duplicates
                                let unique: std::collections::HashSet<_> = players.iter().collect();
                                prop_assert_eq!(unique.len(), players.len(),
                                    "duplicate players in LobbyUpdated: {:?}", players);
                                // Leader is always the first player (creator)
                                prop_assert_eq!(leader, &names[0],
                                    "leader should always be the first player");
                                player_counts_in_updates.push(players.len());
                                leaders_seen.push(leader.clone());
                            }
                            other => {
                                return Err(TestCaseError::fail(format!("unexpected message: {:?}", other)));
                            }
                        }
                    }

                    // All leaders seen should be the first player
                    for leader in &leaders_seen {
                        prop_assert_eq!(leader, &names[0], "leader invariant violated");
                    }
                }

                cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
                Ok::<(), TestCaseError>(())
            }).unwrap();
        }
    }

    // =========================================================================
    // Unit tests: lobby leaving and disconnection (Task 5.3)
    // =========================================================================

    // -------------------------------------------------------------------------
    // Test: LeaveLobby removes player and sends LobbyUpdated to remaining
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn leave_lobby_removes_player_and_sends_lobby_updated() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(100);
        let addr_b = addr(101);

        // A creates lobby
        let name_a = connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins
        let _name_b = connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B) and LobbyUpdated (A)
        for _ in 0..2 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // B leaves
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::LeaveLobby,
        }).await.unwrap();

        // A should receive LobbyUpdated with only A
        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_a);
        match msg.message {
            ServerMessage::LobbyUpdated { leader, players } => {
                assert_eq!(leader, name_a);
                assert_eq!(players, vec![name_a.clone()]);
            }
            other => panic!("expected LobbyUpdated, got {:?}", other),
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: LeaveLobby transfers leadership when leader leaves
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn leave_lobby_transfers_leadership() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(110);
        let addr_b = addr(111);

        // A creates lobby
        let _name_a = connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins
        let name_b = connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B) and LobbyUpdated (A)
        for _ in 0..2 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // A (leader) leaves
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::LeaveLobby,
        }).await.unwrap();

        // B should receive LobbyUpdated with B as leader
        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_b);
        match msg.message {
            ServerMessage::LobbyUpdated { leader, players } => {
                assert_eq!(leader, name_b, "B should be the new leader");
                assert_eq!(players, vec![name_b.clone()]);
            }
            other => panic!("expected LobbyUpdated, got {:?}", other),
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: last player leaves lobby — lobby removed silently
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn leave_lobby_last_player_removes_lobby() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(120);

        // A creates lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");

        // A leaves (last player)
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::LeaveLobby,
        }).await.unwrap();

        // No LobbyUpdated should be sent — verify by checking no message arrives
        let result = timeout(Duration::from_millis(200), outbound_rx.recv()).await;
        assert!(result.is_err(), "expected no message after last player leaves, but got one");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: LeaveLobby during game sends Error
    // TODO: This test requires task 7.2 to set lobby state to InGame.
    // Skipping until start_game_session is implemented.
    // -------------------------------------------------------------------------
    // #[tokio::test]
    // async fn leave_lobby_during_game_sends_error() { ... }

    // -------------------------------------------------------------------------
    // Test: disconnect in PreLobby removes player silently
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_in_pre_lobby_removes_player() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(130);

        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;

        // Disconnect
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_a,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // No outbound messages expected
        let result = timeout(Duration::from_millis(200), outbound_rx.recv()).await;
        assert!(result.is_err(), "expected no message after PreLobby disconnect");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: disconnect in lobby applies leave rules
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_in_lobby_applies_leave_rules() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(140);
        let addr_b = addr(141);

        // A creates lobby
        let name_a = connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins
        let _name_b = connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B) and LobbyUpdated (A)
        for _ in 0..2 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // B disconnects
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_b,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // A should receive LobbyUpdated with only A
        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_a);
        match msg.message {
            ServerMessage::LobbyUpdated { leader, players } => {
                assert_eq!(leader, name_a);
                assert_eq!(players, vec![name_a.clone()]);
            }
            other => panic!("expected LobbyUpdated, got {:?}", other),
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: disconnect during negotiation — no player state
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_during_negotiation_no_player_state() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(150);

        // Connect but don't identify
        event_tx.send(LobbyEvent::ClientConnected { address: addr_a }).await.unwrap();

        // Disconnect
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_a,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // No outbound messages expected
        let result = timeout(Duration::from_millis(200), outbound_rx.recv()).await;
        assert!(result.is_err(), "expected no message after negotiation disconnect");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // =========================================================================
    // Property 5: Disconnection cleanup
    // Feature: go-fish-lobby-and-game, Property 5: Disconnection cleanup
    // Validates: Requirements 8.1–8.4, 9.1–9.4
    // =========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_disconnection_cleanup(state in 0usize..3usize) {
            prop_async!({
                // state: 0 = negotiating, 1 = pre-lobby, 2 = in-lobby
                let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);

                let disconnecting_addr: SocketAddr = "127.0.0.5:60000".parse().unwrap();
                let other_addr: SocketAddr = "127.0.0.5:60001".parse().unwrap();

                match state {
                    0 => {
                        // Negotiating: connect but don't identify
                        event_tx.send(LobbyEvent::ClientConnected { address: disconnecting_addr }).await.unwrap();
                    }
                    1 => {
                        // Pre-lobby: connect and identify
                        connect_and_identify(&event_tx, &mut outbound_rx, disconnecting_addr).await;
                    }
                    2 => {
                        // In-lobby: connect, identify, create lobby, have another player join
                        connect_and_identify(&event_tx, &mut outbound_rx, disconnecting_addr).await;
                        event_tx.send(LobbyEvent::ClientMessage {
                            address: disconnecting_addr,
                            message: ClientMessage::CreateLobby,
                        }).await.unwrap();
                        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
                            .await.expect("timed out").expect("channel closed").message
                        {
                            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
                            other => return Err(TestCaseError::fail(format!("expected LobbyJoined, got {:?}", other))),
                        };

                        // Another player joins so we can verify LobbyUpdated is sent to them
                        connect_and_identify(&event_tx, &mut outbound_rx, other_addr).await;
                        event_tx.send(LobbyEvent::ClientMessage {
                            address: other_addr,
                            message: ClientMessage::JoinLobby(lobby_id.clone()),
                        }).await.unwrap();
                        // Drain LobbyJoined (other) and LobbyUpdated (disconnecting)
                        for _ in 0..2 {
                            timeout(Duration::from_secs(2), outbound_rx.recv())
                                .await.expect("timed out").expect("channel closed");
                        }
                    }
                    _ => unreachable!(),
                }

                // Send disconnect
                event_tx.send(LobbyEvent::ClientDisconnected {
                    address: disconnecting_addr,
                    reason: crate::connection::DisconnectReason::Clean,
                }).await.unwrap();

                // Give the manager a moment to process
                tokio::time::sleep(Duration::from_millis(50)).await;

                let mut messages: Vec<LobbyOutboundMessage> = Vec::new();
                while let Ok(Some(msg)) = timeout(Duration::from_millis(50), outbound_rx.recv()).await {
                    messages.push(msg);
                }

                // Assert: no message was sent to the disconnecting address
                for msg in &messages {
                    prop_assert_ne!(msg.address, disconnecting_addr,
                        "should not send message to disconnected player, got {:?}", msg.message);
                }

                // Assert: if in-lobby with other players, those players receive LobbyUpdated
                if state == 2 {
                    let lobby_updated_to_other = messages.iter().any(|m| {
                        m.address == other_addr && matches!(m.message, ServerMessage::LobbyUpdated { .. })
                    });
                    prop_assert!(lobby_updated_to_other,
                        "remaining lobby player should receive LobbyUpdated after disconnect");
                }

                cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
                Ok::<(), TestCaseError>(())
            }).unwrap();
        }
    }

    // =========================================================================
    // Unit tests: game start (Task 7.3)
    // =========================================================================

    // -------------------------------------------------------------------------
    // Test: StartGame from non-leader sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn start_game_non_leader_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(200);
        let addr_b = addr(201);

        // A creates lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B) and LobbyUpdated (A)
        for _ in 0..2 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // B (non-leader) sends StartGame
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::StartGame,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_b);
        assert!(
            matches!(msg.message, ServerMessage::Error(ref e) if e == "only the leader can start the game"),
            "expected Error('only the leader can start the game'), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: StartGame with fewer than 2 players sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn start_game_insufficient_players_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(210);

        // A creates lobby (alone)
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");

        // A (leader, alone) sends StartGame
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::StartGame,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_a);
        assert!(
            matches!(msg.message, ServerMessage::Error(ref e) if e == "need at least 2 players to start"),
            "expected Error('need at least 2 players to start'), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: StartGame (leader, ≥2 players) sends GameStarted + PlayerIdentity + HandState + PlayerTurn to all
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn start_game_sends_game_started_and_initial_state() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(4);
        let addr_a = addr(220);
        let addr_b = addr(221);

        // A creates lobby
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B) and LobbyUpdated (A)
        for _ in 0..2 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // A (leader) sends StartGame
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::StartGame,
        }).await.unwrap();

        // Collect 8 messages: for each of A and B: GameStarted, PlayerIdentity, HandState, PlayerTurn
        let mut msgs: Vec<LobbyOutboundMessage> = Vec::new();
        for _ in 0..8 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out waiting for game start messages")
                .expect("channel closed");
            msgs.push(msg);
        }

        for &player_addr in &[addr_a, addr_b] {
            let player_msgs: Vec<&ServerMessage> = msgs.iter()
                .filter(|m| m.address == player_addr)
                .map(|m| &m.message)
                .collect();
            assert_eq!(player_msgs.len(), 4, "each player should receive 4 messages");
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::GameStarted)),
                "player {:?} should receive GameStarted", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::PlayerIdentity(_))),
                "player {:?} should receive PlayerIdentity", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::HandState(_))),
                "player {:?} should receive HandState", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::PlayerTurn(_))),
                "player {:?} should receive PlayerTurn", player_addr);
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: auto-start when lobby full starts game (sends GameStarted to all)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn auto_start_when_lobby_full_starts_game() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(230);
        let addr_b = addr(231);

        // A creates lobby (max_players=2)
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins — fills lobby, triggers auto-start
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Collect all messages: LobbyJoined(B), LobbyUpdated(A), then game start messages
        // Total: 2 (lobby) + 8 (game: 4 per player) = 10
        let mut msgs: Vec<LobbyOutboundMessage> = Vec::new();
        for _ in 0..10 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out waiting for auto-start messages")
                .expect("channel closed");
            msgs.push(msg);
        }

        // Both A and B should receive GameStarted
        let a_got_game_started = msgs.iter().any(|m| m.address == addr_a && matches!(m.message, ServerMessage::GameStarted));
        let b_got_game_started = msgs.iter().any(|m| m.address == addr_b && matches!(m.message, ServerMessage::GameStarted));
        assert!(a_got_game_started, "player A should receive GameStarted on auto-start");
        assert!(b_got_game_started, "player B should receive GameStarted on auto-start");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // Drain startup messages and return (name_a, name_b, whose_turn_addr, hand_state_a, hand_state_b)
    async fn setup_two_player_game_with_state(
        event_tx: &mpsc::Sender<LobbyEvent>,
        outbound_rx: &mut mpsc::Receiver<LobbyOutboundMessage>,
        addr_a: SocketAddr,
        addr_b: SocketAddr,
    ) -> (String, String, SocketAddr, go_fish_web::HandState, go_fish_web::HandState) {
        // A creates lobby
        let name_a = connect_and_identify(event_tx, outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins — fills lobby (max=2), auto-starts
        let name_b = connect_and_identify(event_tx, outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id),
        }).await.unwrap();

        // Drain all startup messages: LobbyJoined(B), LobbyUpdated(A), then 8 game-start messages
        let mut whose_turn: Option<SocketAddr> = None;
        let mut hand_state_a: Option<go_fish_web::HandState> = None;
        let mut hand_state_b: Option<go_fish_web::HandState> = None;
        for _ in 0..10 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out draining startup").expect("channel closed");
            match &msg.message {
                ServerMessage::PlayerTurn(go_fish_web::PlayerTurnValue::YourTurn) => {
                    whose_turn = Some(msg.address);
                }
                ServerMessage::HandState(hs) if msg.address == addr_a => {
                    hand_state_a = Some(hs.clone());
                }
                ServerMessage::HandState(hs) if msg.address == addr_b => {
                    hand_state_b = Some(hs.clone());
                }
                _ => {}
            }
        }

        let whose_turn = whose_turn.expect("should have received YourTurn");
        let hand_state_a = hand_state_a.expect("should have received HandState for A");
        let hand_state_b = hand_state_b.expect("should have received HandState for B");

        (name_a, name_b, whose_turn, hand_state_a, hand_state_b)
    }

    // =========================================================================
    // Unit tests: in-game play (Task 8.2)
    // =========================================================================

    // -------------------------------------------------------------------------
    // Test: Hook when not player's turn sends HookError(NotYourTurn)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn hook_not_your_turn_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(300);
        let addr_b = addr(301);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        // Determine who goes second
        let (second_addr, second_name, first_name) = if whose_turn == addr_a {
            (addr_b, name_b.clone(), name_a.clone())
        } else {
            (addr_a, name_a.clone(), name_b.clone())
        };

        // Second player sends a Hook (not their turn)
        // Use any rank from second player's hand
        let second_hand = if second_addr == addr_a { &hand_state_a } else { &hand_state_b };
        let rank = second_hand.hand.books.first()
            .map(|b| b.rank)
            .unwrap_or(go_fish::Rank::Two);

        event_tx.send(LobbyEvent::ClientMessage {
            address: second_addr,
            message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                target_name: first_name,
                rank,
            }),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, second_addr);
        assert!(
            matches!(msg.message, ServerMessage::HookError(go_fish_web::HookError::NotYourTurn)),
            "expected HookError(NotYourTurn), got {:?}", msg.message
        );

        let _ = second_name;
        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: Hook targeting unknown player sends HookError(UnknownPlayer)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn hook_unknown_player_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(310);
        let addr_b = addr(311);

        let (_name_a, _name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        let current_hand = if whose_turn == addr_a { &hand_state_a } else { &hand_state_b };
        let rank = current_hand.hand.books.first()
            .map(|b| b.rank)
            .unwrap_or(go_fish::Rank::Two);

        event_tx.send(LobbyEvent::ClientMessage {
            address: whose_turn,
            message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                target_name: "nonexistent".to_string(),
                rank,
            }),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, whose_turn);
        assert!(
            matches!(&msg.message, ServerMessage::HookError(go_fish_web::HookError::UnknownPlayer(name)) if name == "nonexistent"),
            "expected HookError(UnknownPlayer(\"nonexistent\")), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: Hook targeting self sends HookError(CannotTargetYourself)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn hook_target_self_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(320);
        let addr_b = addr(321);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        let (current_name, current_hand) = if whose_turn == addr_a {
            (name_a.clone(), &hand_state_a)
        } else {
            (name_b.clone(), &hand_state_b)
        };

        let rank = current_hand.hand.books.first()
            .map(|b| b.rank)
            .unwrap_or(go_fish::Rank::Two);

        event_tx.send(LobbyEvent::ClientMessage {
            address: whose_turn,
            message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                target_name: current_name,
                rank,
            }),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, whose_turn);
        assert!(
            matches!(msg.message, ServerMessage::HookError(go_fish_web::HookError::CannotTargetYourself)),
            "expected HookError(CannotTargetYourself), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: Hook for rank not in hand sends HookError(YouDoNotHaveRank)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn hook_rank_not_in_hand_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(330);
        let addr_b = addr(331);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        let (current_hand, target_name) = if whose_turn == addr_a {
            (&hand_state_a, name_b.clone())
        } else {
            (&hand_state_b, name_a.clone())
        };

        // Find a rank the current player does NOT hold
        let held_ranks: Vec<go_fish::Rank> =
            current_hand.hand.books.iter().map(|b| b.rank).collect();
        let all_ranks = [
            go_fish::Rank::Two, go_fish::Rank::Three, go_fish::Rank::Four,
            go_fish::Rank::Five, go_fish::Rank::Six, go_fish::Rank::Seven,
            go_fish::Rank::Eight, go_fish::Rank::Nine, go_fish::Rank::Ten,
            go_fish::Rank::Jack, go_fish::Rank::Queen, go_fish::Rank::King,
            go_fish::Rank::Ace,
        ];
        let missing_rank = all_ranks.iter().copied()
            .find(|r| !held_ranks.contains(r))
            .expect("player should be missing at least one rank");

        event_tx.send(LobbyEvent::ClientMessage {
            address: whose_turn,
            message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                target_name,
                rank: missing_rank,
            }),
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, whose_turn);
        assert!(
            matches!(&msg.message, ServerMessage::HookError(go_fish_web::HookError::YouDoNotHaveRank(r)) if *r == missing_rank),
            "expected HookError(YouDoNotHaveRank({:?})), got {:?}", missing_rank, msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: valid Hook broadcasts HookAndResult, HandState, and PlayerTurn to all
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn valid_hook_broadcasts_result_and_updates_state() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(340);
        let addr_b = addr(341);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        let (current_hand, target_name, other_addr) = if whose_turn == addr_a {
            (&hand_state_a, name_b.clone(), addr_b)
        } else {
            (&hand_state_b, name_a.clone(), addr_a)
        };

        // Find a rank the current player DOES hold
        let rank = current_hand.hand.books.first()
            .map(|b| b.rank)
            .expect("current player should have at least one rank");

        event_tx.send(LobbyEvent::ClientMessage {
            address: whose_turn,
            message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                target_name,
                rank,
            }),
        }).await.unwrap();

        // Collect messages: 2 HookAndResult + 2 HandState + 2 PlayerTurn = 6 messages
        let mut msgs: Vec<LobbyOutboundMessage> = Vec::new();
        for _ in 0..6 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out waiting for hook result messages")
                .expect("channel closed");
            msgs.push(msg);
        }

        for &player_addr in &[whose_turn, other_addr] {
            let player_msgs: Vec<&ServerMessage> = msgs.iter()
                .filter(|m| m.address == player_addr)
                .map(|m| &m.message)
                .collect();
            assert_eq!(player_msgs.len(), 3, "each player should receive 3 messages after hook");
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::HookAndResult(_))),
                "player {:?} should receive HookAndResult", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::HandState(_))),
                "player {:?} should receive HandState", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::PlayerTurn(_))),
                "player {:?} should receive PlayerTurn", player_addr);
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // =========================================================================
    // Property 4: Game session isolation
    // Feature: go-fish-lobby-and-game, Property 4: Game session isolation
    // Validates: Requirement 6 (isolation between concurrent games)
    // =========================================================================
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_game_session_isolation(_seed in 0u32..100u32) {
            prop_async!({
                let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);

                // Lobby 1: players at ports 11000-11001
                let addr_l1_a: SocketAddr = "127.0.2.1:11000".parse().unwrap();
                let addr_l1_b: SocketAddr = "127.0.2.1:11001".parse().unwrap();
                // Lobby 2: players at ports 11002-11003
                let addr_l2_a: SocketAddr = "127.0.2.1:11002".parse().unwrap();
                let addr_l2_b: SocketAddr = "127.0.2.1:11003".parse().unwrap();

                // Start lobby 1
                let (name_l1_a, name_l1_b, whose_turn_l1, hand_l1_a, hand_l1_b) =
                    setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_l1_a, addr_l1_b).await;

                // Start lobby 2
                let (_name_l2_a, _name_l2_b, _whose_turn_l2, _hand_l2_a, _hand_l2_b) =
                    setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_l2_a, addr_l2_b).await;

                // Process a hook in lobby 1
                let (current_hand_l1, target_name_l1) = if whose_turn_l1 == addr_l1_a {
                    (&hand_l1_a, name_l1_b.clone())
                } else {
                    (&hand_l1_b, name_l1_a.clone())
                };

                let rank = current_hand_l1.hand.books.first()
                    .map(|b| b.rank)
                    .expect("current player should have at least one rank");

                event_tx.send(LobbyEvent::ClientMessage {
                    address: whose_turn_l1,
                    message: ClientMessage::Hook(go_fish_web::ClientHookRequest {
                        target_name: target_name_l1,
                        rank,
                    }),
                }).await.unwrap();

                // Collect all messages that arrive
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut messages: Vec<LobbyOutboundMessage> = Vec::new();
                while let Ok(Some(msg)) = timeout(Duration::from_millis(50), outbound_rx.recv()).await {
                    messages.push(msg);
                }

                // Assert: lobby 2 players receive NO messages as a result of the hook in lobby 1
                let lobby2_addrs = [addr_l2_a, addr_l2_b];
                for msg in &messages {
                    prop_assert!(
                        !lobby2_addrs.contains(&msg.address),
                        "lobby 2 player {:?} should not receive messages from lobby 1 hook, got {:?}",
                        msg.address, msg.message
                    );
                }

                // Assert: lobby 1 players DO receive messages (HookAndResult, HandState, PlayerTurn)
                let lobby1_msgs: Vec<_> = messages.iter()
                    .filter(|m| m.address == addr_l1_a || m.address == addr_l1_b)
                    .collect();
                prop_assert!(!lobby1_msgs.is_empty(), "lobby 1 players should receive hook result messages");

                cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
                Ok::<(), TestCaseError>(())
            }).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // Test: LeaveLobby during game sends Error
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn leave_lobby_during_game_sends_error() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(240);
        let addr_b = addr(241);

        // A creates lobby (max_players=2)
        connect_and_identify(&event_tx, &mut outbound_rx, addr_a).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();
        let lobby_id = match timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed").message
        {
            ServerMessage::LobbyJoined { lobby_id, .. } => lobby_id,
            other => panic!("expected LobbyJoined, got {:?}", other),
        };

        // B joins — fills lobby, triggers auto-start
        connect_and_identify(&event_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Drain all auto-start messages (2 lobby + 8 game = 10)
        for _ in 0..10 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out draining auto-start messages")
                .expect("channel closed");
        }

        // A tries to leave during game
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::LeaveLobby,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        assert_eq!(msg.address, addr_a);
        assert!(
            matches!(msg.message, ServerMessage::Error(ref e) if e == "cannot leave during game"),
            "expected Error('cannot leave during game'), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // =========================================================================
    // Unit tests: game completion and disconnection (Task 9.3)
    // =========================================================================

    // -------------------------------------------------------------------------
    // Test: disconnect during game ends game and sends GameResult to survivors only
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_during_game_ends_game_and_sends_game_result() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(400);
        let addr_b = addr(401);

        // Start a 2-player game
        setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        // Player B disconnects
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_b,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // Collect messages — player A should receive GameResult, player B should NOT
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let mut messages: Vec<LobbyOutboundMessage> = Vec::new();
        while let Ok(Some(msg)) = timeout(Duration::from_millis(100), outbound_rx.recv()).await {
            messages.push(msg);
        }

        // Player A should receive GameResult
        let a_got_game_result = messages.iter().any(|m| {
            m.address == addr_a && matches!(m.message, ServerMessage::GameResult(_))
        });
        assert!(a_got_game_result, "player A should receive GameResult after B disconnects");

        // Player B should NOT receive GameResult (they disconnected)
        let b_got_game_result = messages.iter().any(|m| {
            m.address == addr_b && matches!(m.message, ServerMessage::GameResult(_))
        });
        assert!(!b_got_game_result, "player B should NOT receive GameResult after disconnecting");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: after game ends, players can create a new lobby (proves PreLobby state)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn after_game_ends_players_can_create_new_lobby() {
        let (event_tx, mut outbound_rx, cmd_tx, _handle) = make_lobby_manager(2);
        let addr_a = addr(410);
        let addr_b = addr(411);

        // Start a 2-player game
        setup_two_player_game_with_state(&event_tx, &mut outbound_rx, addr_a, addr_b).await;

        // Player B disconnects (ends the game)
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_b,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // Drain the GameResult message sent to player A
        let game_result_msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out waiting for GameResult").expect("channel closed");
        assert!(
            matches!(game_result_msg.message, ServerMessage::GameResult(_)),
            "expected GameResult, got {:?}", game_result_msg.message
        );

        // Player A sends CreateLobby — should succeed (proves they're back in PreLobby)
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_a,
            message: ClientMessage::CreateLobby,
        }).await.unwrap();

        let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out waiting for LobbyJoined").expect("channel closed");

        assert_eq!(msg.address, addr_a);
        assert!(
            matches!(msg.message, ServerMessage::LobbyJoined { .. }),
            "expected LobbyJoined (player A should be back in PreLobby), got {:?}", msg.message
        );

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }
}
