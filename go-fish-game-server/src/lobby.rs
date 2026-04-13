use crate::connection::LobbyEvent;
use crate::{BotConfig, SimpleBotConfig};
use go_fish::bots::{Bot, BotObservation, OpponentView};
use go_fish_web::{ClientHookRequest, ClientMessage, LobbyLeftReason, LobbyPlayer, ServerMessage};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, instrument, warn};

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
    /// Outbound channel for sending messages to this client.
    pub sender: mpsc::Sender<ServerMessage>,
}

// ── Lobby-phase slots ─────────────────────────────────────────────────────────

/// A human player waiting in the lobby (pre-game).
#[derive(Debug)]
pub(crate) struct HumanSlot {
    address: SocketAddr,
    name: String,
}

/// A bot configuration waiting in the lobby (pre-game).
#[derive(Debug)]
pub(crate) struct BotSlot {
    name: String,
    bot_type: go_fish_web::BotType,
}

// ── Game-phase participant ────────────────────────────────────────────────────

/// A unified game participant — human or bot — identified by id and reachable via sender.
#[derive(Debug)]
pub struct Participant {
    pub id: go_fish::PlayerId,
    pub name: String,
    pub sender: mpsc::Sender<ServerMessage>,
}

// ── Game session ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct GameSession {
    pub game: go_fish::Game,
    pub id_to_name: HashMap<go_fish::PlayerId, String>,
    pub name_to_id: HashMap<String, go_fish::PlayerId>,
    pub participants: Vec<Participant>,
}

impl GameSession {
    async fn broadcast(&self, msg: ServerMessage) {
        for p in &self.participants {
            let _ = p.sender.send(msg.clone()).await;
        }
    }


}

// ── Lobby ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum LobbyState {
    Waiting {
        /// Human players in join order; [0] is always the leader.
        connected_players: Vec<HumanSlot>,
        /// Bot configurations added by the leader, in add order.
        pending_bots: Vec<BotSlot>,
    },
    InGame(GameSession),
}

#[derive(Debug)]
pub struct Lobby {
    pub lobby_id: String,
    pub max_players: usize,
    pub(crate) state: LobbyState,
}

impl Lobby {
    fn new(lobby_id: String, max_players: usize) -> Self {
        Lobby {
            lobby_id,
            max_players,
            state: LobbyState::Waiting {
                connected_players: Vec::new(),
                pending_bots: Vec::new(),
            },
        }
    }

    /// Total slots occupied (humans + pending bots). Only valid in Waiting state.
    fn participant_count(&self) -> usize {
        match &self.state {
            LobbyState::Waiting { connected_players, pending_bots } => {
                connected_players.len() + pending_bots.len()
            }
            LobbyState::InGame(_) => 0,
        }
    }

    /// Build the Vec<LobbyPlayer> view of current waiting occupants.
    fn lobby_player_list(&self) -> Vec<LobbyPlayer> {
        match &self.state {
            LobbyState::Waiting { connected_players, pending_bots } => {
                let mut list: Vec<LobbyPlayer> = connected_players.iter()
                    .map(|s| LobbyPlayer::Human { name: s.name.clone() })
                    .collect();
                list.extend(pending_bots.iter().map(|b| LobbyPlayer::Bot {
                    name: b.name.clone(),
                    bot_type: b.bot_type.clone(),
                }));
                list
            }
            LobbyState::InGame(_) => Vec::new(),
        }
    }
}

// ── LobbyCommand ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LobbyCommand {
    Shutdown,
}

// ── ThinkingTimeConfig ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThinkingTimeConfig {
    pub min_ms: u64,
    pub max_ms: u64,
}

// ── LobbyManager ─────────────────────────────────────────────────────────────

pub struct LobbyManager {
    negotiating: HashSet<SocketAddr>,
    players: HashMap<SocketAddr, PlayerRecord>,
    names_in_use: HashSet<String>,
    lobbies: HashMap<String, Lobby>,
    lobby_max_players: usize,
    event_rx: mpsc::Receiver<LobbyEvent>,
    /// Clone used to inject LobbyEvent::Hook from BotDriver tasks.
    event_tx: mpsc::Sender<LobbyEvent>,
    command_rx: mpsc::Receiver<LobbyCommand>,
    thinking_time: ThinkingTimeConfig,
    simple_bot_config: SimpleBotConfig,
}

impl LobbyManager {
    pub fn new(
        event_rx: mpsc::Receiver<LobbyEvent>,
        command_rx: mpsc::Receiver<LobbyCommand>,
        lobby_max_players: usize,
        event_tx: mpsc::Sender<LobbyEvent>,
        bot_config: BotConfig,
    ) -> Self {
        LobbyManager {
            negotiating: HashSet::new(),
            players: HashMap::new(),
            names_in_use: HashSet::new(),
            lobbies: HashMap::new(),
            lobby_max_players,
            event_rx,
            event_tx,
            command_rx,
            thinking_time: ThinkingTimeConfig {
                min_ms: bot_config.thinking_time_min_ms,
                max_ms: bot_config.thinking_time_max_ms,
            },
            simple_bot_config: bot_config.simple_bot,
        }
    }

    pub fn command_channel() -> (mpsc::Sender<LobbyCommand>, mpsc::Receiver<LobbyCommand>) {
        mpsc::channel(8)
    }

    #[instrument(skip(self))]
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

    #[instrument(level = "debug", skip(self))]
    async fn handle_event(&mut self, event: LobbyEvent) {
        match event {
            LobbyEvent::ClientConnected { address, message_tx } => {
                self.negotiating.insert(address);
                // Store the sender keyed by address so we can use it after identity
                // negotiation completes. We keep a temporary entry until identity is confirmed.
                self.players.insert(address, PlayerRecord {
                    name: String::new(),
                    address,
                    phase: ClientPhase::IdentityNegotiation,
                    sender: message_tx,
                });
                debug!(%address, "client entered identity negotiation phase");
            }

            LobbyEvent::ClientMessage { address, message } => {
                // Reject non-Identity messages during negotiation
                if self.negotiating.contains(&address) {
                    if !matches!(message, ClientMessage::Identity) {
                        self.send(address, ServerMessage::Error("must send Identity first".to_string())).await;
                        return;
                    }
                    // Assign a unique name and update the existing PlayerRecord (sender already stored)
                    let mut name = random_fish_name();
                    while self.names_in_use.contains(&name) {
                        name = random_fish_name();
                    }
                    self.negotiating.remove(&address);
                    if let Some(record) = self.players.get_mut(&address) {
                        record.name = name.clone();
                        record.phase = ClientPhase::PreLobby;
                    }
                    self.names_in_use.insert(name.clone());
                    self.send(address, ServerMessage::PlayerIdentity(name.clone())).await;
                    info!(%address, name = %name, "player identity assigned");
                    return;
                }

                // Reject duplicate Identity from already-identified players
                if self.players.contains_key(&address)
                    && matches!(message, ClientMessage::Identity) {
                        self.send(address, ServerMessage::Error("already identified".to_string())).await;
                        return;
                    }

                // Route to player message handler
                self.handle_player_message(address, message).await;
            }

            LobbyEvent::ClientDisconnected { address, .. } => {
                // Always remove the player record; if still negotiating, remove from that set too
                self.negotiating.remove(&address);
                if let Some(record) = self.players.get(&address) {
                    let phase = record.phase.clone();
                    match phase {
                        ClientPhase::IdentityNegotiation | ClientPhase::PreLobby => {}
                        ClientPhase::InLobby { lobby_id } => {
                            let msgs = self.remove_player_from_lobby(address, &lobby_id);
                            for (addr, msg) in msgs {
                                self.send(addr, msg).await;
                            }
                        }
                        ClientPhase::InGame { lobby_id } => {
                            info!(%address, lobby_id = %lobby_id, "player disconnected mid-game, ending session");
                            // Remove disconnecting player from participants so they don't receive GameAborted
                            let disc_name = self.players.get(&address).map(|r| r.name.clone());
                            if let Some(lobby) = self.lobbies.get_mut(&lobby_id)
                                && let LobbyState::InGame(session) = &mut lobby.state
                                    && let Some(name) = &disc_name {
                                        session.participants.retain(|p| &p.name != name);
                                    }
                            self.end_game_session(lobby_id, true).await;
                        }
                    }
                    let name = self.players[&address].name.clone();
                    self.players.remove(&address);
                    if !name.is_empty() {
                        self.names_in_use.remove(&name);
                    }
                }
            }

            LobbyEvent::Hook { lobby_id, player_name, request } => {
                self.process_hook(lobby_id, player_name, request).await;
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
                let mut lobby_id = random_water_name();
                while self.lobbies.contains_key(&lobby_id) {
                    lobby_id = random_water_name();
                }

                let name = self.players[&address].name.clone();

                // Create lobby with the player as first connected member (leader)
                let mut lobby = Lobby::new(lobby_id.clone(), self.lobby_max_players);
                if let LobbyState::Waiting { connected_players, .. } = &mut lobby.state {
                    connected_players.push(HumanSlot { address, name: name.clone() });
                }
                self.lobbies.insert(lobby_id.clone(), lobby);

                // Update player phase
                if let Some(record) = self.players.get_mut(&address) {
                    record.phase = ClientPhase::InLobby { lobby_id: lobby_id.clone() };
                }

                info!(lobby_id = %lobby_id, leader = %name, "lobby created");

                self.send(address, ServerMessage::LobbyJoined {
                    lobby_id,
                    leader: name.clone(),
                    players: vec![LobbyPlayer::Human { name }],
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
                    if lobby.participant_count() >= lobby.max_players {
                        self.send(address, ServerMessage::Error("lobby is full".to_string())).await;
                        return;
                    }
                }

                let joining_name = self.players[&address].name.clone();

                // Add player to lobby
                if let Some(lobby) = self.lobbies.get_mut(&lobby_id)
                    && let LobbyState::Waiting { connected_players, .. } = &mut lobby.state {
                        connected_players.push(HumanSlot { address, name: joining_name.clone() });
                    }

                // Update player phase
                if let Some(record) = self.players.get_mut(&address) {
                    record.phase = ClientPhase::InLobby { lobby_id: lobby_id.clone() };
                }

                // Build player list, leader name, and other player addresses
                let (leader_name, player_list, other_addrs, max_players, total) = {
                    let lobby = self.lobbies.get(&lobby_id).unwrap();
                    let (leader_name, other_addrs) = match &lobby.state {
                        LobbyState::Waiting { connected_players, .. } => {
                            let leader = connected_players[0].name.clone();
                            let others: Vec<SocketAddr> = connected_players.iter()
                                .filter(|s| s.address != address)
                                .map(|s| s.address)
                                .collect();
                            (leader, others)
                        }
                        _ => return,
                    };
                    let player_list = lobby.lobby_player_list();
                    (leader_name, player_list, other_addrs, lobby.max_players, lobby.participant_count())
                };

                info!(lobby_id = %lobby_id, player = %joining_name, "player joined lobby");

                // Send LobbyJoined to joining player
                self.send(address, ServerMessage::LobbyJoined {
                    lobby_id: lobby_id.clone(),
                    leader: leader_name.clone(),
                    players: player_list.clone(),
                    max_players,
                }).await;

                // Send LobbyUpdated to all other players
                for other_addr in other_addrs {
                    self.send(other_addr, ServerMessage::LobbyUpdated {
                        leader: leader_name.clone(),
                        players: player_list.clone(),
                    }).await;
                }

                // Auto-start if lobby is now full
                if total >= max_players {
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
                if let Some(lobby) = self.lobbies.get(&lobby_id)
                    && matches!(lobby.state, LobbyState::InGame(_)) {
                        self.send(address, ServerMessage::Error("cannot leave during game".to_string())).await;
                        return;
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

            ClientMessage::AddBot { bot_type } => {
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InLobby { lobby_id }) => lobby_id,
                    _ => {
                        self.send(address, ServerMessage::Error("not in a lobby".to_string())).await;
                        return;
                    }
                };

                // Only the leader can add bots
                let is_leader = self.lobbies.get(&lobby_id).map(|l| {
                    match &l.state {
                        LobbyState::Waiting { connected_players, .. } => {
                            connected_players.first().map(|s| s.address) == Some(address)
                        }
                        _ => false,
                    }
                }).unwrap_or(false);
                if !is_leader {
                    self.send(address, ServerMessage::Error("only the leader can add bots".to_string())).await;
                    return;
                }

                let (at_capacity, bot_count) = self.lobbies.get(&lobby_id).map(|l| {
                    let count = match &l.state {
                        LobbyState::Waiting { pending_bots, .. } => pending_bots.len(),
                        _ => 0,
                    };
                    (l.participant_count() >= l.max_players, count)
                }).unwrap_or((true, 0));
                if at_capacity {
                    self.send(address, ServerMessage::Error("lobby is full".to_string())).await;
                    return;
                }

                let bot_name = format!("Bot {}", bot_count + 1);
                if let Some(lobby) = self.lobbies.get_mut(&lobby_id)
                    && let LobbyState::Waiting { pending_bots, .. } = &mut lobby.state {
                        pending_bots.push(BotSlot { name: bot_name, bot_type });
                    }

                let (leader_name, player_list) = self.lobby_leader_and_list(&lobby_id);
                self.broadcast_lobby_updated(&lobby_id, leader_name, player_list).await;
            }

            ClientMessage::RemoveBot => {
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InLobby { lobby_id }) => lobby_id,
                    _ => {
                        self.send(address, ServerMessage::Error("not in a lobby".to_string())).await;
                        return;
                    }
                };

                // Only the leader can remove bots
                let is_leader = self.lobbies.get(&lobby_id).map(|l| {
                    match &l.state {
                        LobbyState::Waiting { connected_players, .. } => {
                            connected_players.first().map(|s| s.address) == Some(address)
                        }
                        _ => false,
                    }
                }).unwrap_or(false);
                if !is_leader {
                    self.send(address, ServerMessage::Error("only the leader can remove bots".to_string())).await;
                    return;
                }

                let removed = if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
                    if let LobbyState::Waiting { pending_bots, .. } = &mut lobby.state {
                        pending_bots.pop().is_some()
                    } else {
                        false
                    }
                } else {
                    false
                };

                if removed {
                    let (leader_name, player_list) = self.lobby_leader_and_list(&lobby_id);
                    self.broadcast_lobby_updated(&lobby_id, leader_name, player_list).await;
                }
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

                // Only the leader can start the game
                let is_leader = self.lobbies.get(&lobby_id).map(|l| {
                    match &l.state {
                        LobbyState::Waiting { connected_players, .. } => {
                            connected_players.first().map(|s| s.address) == Some(address)
                        }
                        _ => false,
                    }
                }).unwrap_or(false);
                if !is_leader {
                    self.send(address, ServerMessage::Error("only the leader can start the game".to_string())).await;
                    return;
                }

                // Need at least 2 participants (humans + bots)
                let total = self.lobbies.get(&lobby_id)
                    .map(|l| l.participant_count())
                    .unwrap_or(0);
                if total < 2 {
                    self.send(address, ServerMessage::Error("need at least 2 players to start".to_string())).await;
                    return;
                }

                self.start_game_session(lobby_id).await;
            }

            ClientMessage::Hook(hook_request) => {
                // Must be in InGame phase; resolve player_name and lobby_id then delegate.
                let phase = self.players.get(&address).map(|r| r.phase.clone());
                let lobby_id = match phase {
                    Some(ClientPhase::InGame { lobby_id }) => lobby_id,
                    _ => {
                        self.send(address, ServerMessage::Error("not in a game".to_string())).await;
                        return;
                    }
                };
                let sender_name = self.players[&address].name.clone();
                self.process_hook(lobby_id, sender_name, hook_request).await;
            }

            ClientMessage::Identity => {
                // Already handled above (duplicate identity)
            }
        }
    }

    /// Shared hook processing path used by both human (`ClientMessage::Hook`) and
    /// bot (`LobbyEvent::Hook`) participants. Validates the move, calls `take_turn`,
    /// and broadcasts personalised `GameSnapshot`s to all participants.
    async fn process_hook(&mut self, lobby_id: String, sender_name: String, hook_request: ClientHookRequest) {
                // Validate and collect data from session (immutable borrow)
                enum HookValidation {
                    Valid { target_player_id: go_fish::PlayerId },
                    Invalid(go_fish_web::HookError),
                    UnknownTarget(String),
                }

        let validation = {
            let lobby = match self.lobbies.get(&lobby_id) {
                Some(l) => l,
                None => return,
            };
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };

            let sender_player_id = match session.name_to_id.get(&sender_name) {
                Some(&id) => id,
                None => return,
            };

            // Validation 1: check it's the sender's turn
            let current_player = session.game.get_current_player();
            if current_player.map(|p| p.id) != Some(sender_player_id) {
                HookValidation::Invalid(go_fish_web::HookError::NotYourTurn)
            } else {
                // Validation 2: check target name exists
                match session.name_to_id.get(&hook_request.target_name) {
                    None => HookValidation::UnknownTarget(hook_request.target_name.clone()),
                    Some(&target_player_id) => {
                        // Validation 3: check target is not self
                        if target_player_id == sender_player_id {
                            HookValidation::Invalid(go_fish_web::HookError::CannotTargetYourself)
                        } else {
                            // Validation 4: check sender holds the rank
                            let current_player = session.game.get_current_player().unwrap();
                            let has_rank = current_player.hand.books.iter().any(|b| b.rank == hook_request.rank);
                            if !has_rank {
                                HookValidation::Invalid(go_fish_web::HookError::YouDoNotHaveRank(hook_request.rank))
                            } else {
                                HookValidation::Valid { target_player_id }
                            }
                        }
                    }
                }
            }
        };

        let target_player_id = match validation {
            HookValidation::Invalid(err) => {
                // Send hook error to the sender by name
                self.send_to_player_by_name(&sender_name, ServerMessage::HookError(err)).await;
                return;
            }
            HookValidation::UnknownTarget(name) => {
                self.send_to_player_by_name(&sender_name, ServerMessage::HookError(go_fish_web::HookError::UnknownPlayer(name))).await;
                return;
            }
            HookValidation::Valid { target_player_id } => target_player_id,
        };

        // Collect participant names and target name before mutable borrow
        let (participant_names, target_name_str) = {
            let lobby = self.lobbies.get(&lobby_id).unwrap();
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };
            let names: Vec<String> = session.participants.iter().map(|p| p.name.clone()).collect();
            let target_name = session.id_to_name[&target_player_id].clone();
            (names, target_name)
        };

        // Process the hook (mutable borrow)
        let result = {
            let lobby = self.lobbies.get_mut(&lobby_id).unwrap();
            let session = match &mut lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };
            let span = tracing::debug_span!("take_turn",
                lobby_id = %lobby_id,
                player = %sender_name,
                rank = ?hook_request.rank,
            );
            let _enter = span.enter();
            session.game.take_turn(go_fish::Hook { target: target_player_id, rank: hook_request.rank })
        };

        let result = match result {
            Ok(r) => r,
            Err(e) => {
                warn!(error = ?e, "take_turn error");
                return;
            }
        };

        // Collect updated game state
        let (game_players, inactive_players, current_player_name, is_finished, deck_size) = {
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
            let deck_size = session.game.deck.len();
            (game_players, inactive_players, current_player_name, session.game.is_finished, deck_size)
        };

        let hook_outcome = go_fish_web::HookOutcome {
            fisher_name: sender_name.clone(),
            target_name: target_name_str,
            rank: hook_request.rank,
            result: result.result.clone(),
        };

        // Send personalised GameSnapshot to each participant
        self.broadcast_snapshots(
            &lobby_id,
            &participant_names,
            &game_players,
            &inactive_players,
            &current_player_name,
            deck_size,
            Some(hook_outcome),
        ).await;

        // If game is finished, end the session
        if is_finished {
            self.end_game_session(lobby_id, false).await;
        }
    }

    /// Send a message to a player identified by name. Used for error replies in `process_hook`.
    async fn send_to_player_by_name(&self, name: &str, msg: ServerMessage) {
        if let Some(record) = self.players.values().find(|r| r.name == name) {
            let _ = record.sender.send(msg).await;
        }
    }

    /// Broadcast personalised GameSnapshots to all participants in a lobby.
    #[allow(clippy::too_many_arguments)]
    async fn broadcast_snapshots(
        &self,
        lobby_id: &str,
        participant_names: &[String],
        game_players: &[go_fish::Player],
        inactive_players: &[go_fish::InactivePlayer],
        current_player_name: &str,
        deck_size: usize,
        hook_outcome: Option<go_fish_web::HookOutcome>,
    ) {
        for name in participant_names {
            let lobby = match self.lobbies.get(lobby_id) {
                Some(l) => l,
                None => return,
            };
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };
            let player_id = match session.name_to_id.get(name) {
                Some(&id) => id,
                None => continue,
            };

            let hand_state = if let Some(gf_player) = game_players.iter().find(|p| p.id == player_id) {
                go_fish_web::HandState {
                    hand: gf_player.hand.clone(),
                    completed_books: gf_player.completed_books.clone(),
                }
            } else if let Some(inactive) = inactive_players.iter().find(|p| p.id == player_id) {
                go_fish_web::HandState {
                    hand: go_fish::Hand::empty(),
                    completed_books: inactive.completed_books.clone(),
                }
            } else {
                continue;
            };

            let opponents: Vec<go_fish_web::OpponentState> = participant_names.iter()
                .filter(|other_name| *other_name != name)
                .filter_map(|other_name| {
                    let other_id = session.name_to_id[other_name];
                    if let Some(p) = game_players.iter().find(|p| p.id == other_id) {
                        Some(go_fish_web::OpponentState {
                            name: other_name.clone(),
                            card_count: p.hand.books.iter().map(|b| b.cards.len()).sum(),
                            completed_books: p.completed_books.clone(),
                        })
                    } else { inactive_players.iter().find(|p| p.id == other_id).map(|p| go_fish_web::OpponentState {
                            name: other_name.clone(),
                            card_count: 0,
                            completed_books: p.completed_books.clone(),
                        }) }
                })
                .collect();

            let snapshot = go_fish_web::GameSnapshot {
                hand_state,
                opponents,
                active_player: current_player_name.to_string(),
                last_hook_outcome: hook_outcome.clone(),
                deck_size,
            };

            if let Some(participant) = session.participants.iter().find(|p| &p.name == name) {
                let _ = participant.sender.send(ServerMessage::GameSnapshot(snapshot)).await;
            }
        }
    }

    async fn end_game_session(&mut self, lobby_id: String, disconnection: bool) {
        let (participant_names, game_result_msg) = {
            let lobby = match self.lobbies.get(&lobby_id) {
                Some(l) => l,
                None => return,
            };
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };

            let participant_names: Vec<String> = session.participants.iter()
                .map(|p| p.name.clone())
                .collect();

            let game_result_msg = if disconnection {
                ServerMessage::GameAborted
            } else {
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

            (participant_names, game_result_msg)
        };

        // Send result/abort to all remaining participants
        if let Some(lobby) = self.lobbies.get(&lobby_id)
            && let LobbyState::InGame(session) = &lobby.state {
                session.broadcast(game_result_msg).await;
            }

        // Transition human players back to PreLobby
        for name in &participant_names {
            if let Some(record) = self.players.values_mut().find(|r| &r.name == name) {
                record.phase = ClientPhase::PreLobby;
            }
        }

        // Reset lobby to empty Waiting state
        if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
            lobby.state = LobbyState::Waiting {
                connected_players: Vec::new(),
                pending_bots: Vec::new(),
            };
        }

        info!(lobby_id = %lobby_id, disconnection, "game session ended");
    }

    /// Remove a human player from a lobby waiting room.
    /// Does NOT update the player's phase — caller is responsible.
    /// Does NOT send any message to the leaving player.
    fn remove_player_from_lobby(&mut self, address: SocketAddr, lobby_id: &str) -> Vec<(SocketAddr, ServerMessage)> {
        let lobby = match self.lobbies.get_mut(lobby_id) {
            Some(l) => l,
            None => return vec![],
        };

        let is_empty = if let LobbyState::Waiting { connected_players, .. } = &mut lobby.state {
            connected_players.retain(|s| s.address != address);
            connected_players.is_empty()
        } else {
            return vec![];
        };

        if is_empty {
            // Last human left — also discard any pending bots and remove the lobby
            self.lobbies.remove(lobby_id);
            return vec![];
        }

        let player_list = self.lobbies[lobby_id].lobby_player_list();
        let (leader_name, other_addrs) = match &self.lobbies[lobby_id].state {
            LobbyState::Waiting { connected_players, .. } => {
                let leader = connected_players[0].name.clone();
                let addrs: Vec<SocketAddr> = connected_players.iter().map(|s| s.address).collect();
                (leader, addrs)
            }
            _ => return vec![],
        };

        let msg = ServerMessage::LobbyUpdated {
            leader: leader_name,
            players: player_list,
        };

        other_addrs.iter().map(|&addr| (addr, msg.clone())).collect()
    }

    /// Helper: get leader name + current LobbyPlayer list for broadcasting updates.
    fn lobby_leader_and_list(&self, lobby_id: &str) -> (String, Vec<LobbyPlayer>) {
        match self.lobbies.get(lobby_id) {
            Some(lobby) => {
                let leader = match &lobby.state {
                    LobbyState::Waiting { connected_players, .. } => {
                        connected_players.first().map(|s| s.name.clone()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                (leader, lobby.lobby_player_list())
            }
            None => (String::new(), Vec::new()),
        }
    }

    /// Broadcast `LobbyUpdated` to all connected players in a lobby.
    async fn broadcast_lobby_updated(&self, lobby_id: &str, leader: String, players: Vec<LobbyPlayer>) {
        let addrs: Vec<SocketAddr> = match self.lobbies.get(lobby_id) {
            Some(lobby) => match &lobby.state {
                LobbyState::Waiting { connected_players, .. } => {
                    connected_players.iter().map(|s| s.address).collect()
                }
                _ => return,
            },
            None => return,
        };
        let msg = ServerMessage::LobbyUpdated { leader, players };
        for addr in addrs {
            self.send(addr, msg.clone()).await;
        }
    }

    async fn start_game_session(&mut self, lobby_id: String) {
        // Extract human and bot slots from Waiting state
        let (human_slots, bot_slots, _max_players) = {
            let lobby = match self.lobbies.get(&lobby_id) {
                Some(l) => l,
                None => return,
            };
            match &lobby.state {
                LobbyState::Waiting { connected_players, pending_bots } => {
                    let humans: Vec<(SocketAddr, String)> = connected_players.iter()
                        .map(|s| (s.address, s.name.clone()))
                        .collect();
                    let bots: Vec<(String, go_fish_web::BotType)> = pending_bots.iter()
                        .map(|b| (b.name.clone(), b.bot_type.clone()))
                        .collect();
                    (humans, bots, lobby.max_players)
                }
                _ => return,
            }
        };

        let total = human_slots.len() + bot_slots.len();
        let deck = go_fish::Deck::new().shuffle();
        let game = go_fish::Game::new(deck, total as u8);

        let mut id_to_name: HashMap<go_fish::PlayerId, String> = HashMap::new();
        let mut name_to_id: HashMap<String, go_fish::PlayerId> = HashMap::new();
        let mut participants: Vec<Participant> = Vec::new();

        // Enumerate humans first, then bots — assigns PlayerId in join order
        for (i, (addr, name)) in human_slots.iter().enumerate() {
            let player_id = go_fish::PlayerId::new(i as u8);
            id_to_name.insert(player_id, name.clone());
            name_to_id.insert(name.clone(), player_id);
            let sender = self.players[addr].sender.clone();
            participants.push(Participant { id: player_id, name: name.clone(), sender });
        }

        let bot_offset = human_slots.len();
        let thinking_time = self.thinking_time.clone();
        let simple_bot_config_memory = self.simple_bot_config.memory_limit;
        let simple_bot_config_error = self.simple_bot_config.error_margin;

        for (i, (bot_name, bot_type)) in bot_slots.iter().enumerate() {
            let player_id = go_fish::PlayerId::new((bot_offset + i) as u8);
            id_to_name.insert(player_id, bot_name.clone());
            name_to_id.insert(bot_name.clone(), player_id);

            let (bot_tx, bot_rx) = mpsc::channel::<ServerMessage>(64);
            participants.push(Participant { id: player_id, name: bot_name.clone(), sender: bot_tx });

            // Construct the bot instance
            let bot: Box<dyn Bot + Send> = match bot_type {
                go_fish_web::BotType::SimpleBot => {
                    use rand::RngExt as _;
                    let seed: u64 = rand::rng().random();
                    Box::new(go_fish::bots::SimpleBot::new(
                        player_id,
                        simple_bot_config_memory,
                        simple_bot_config_error,
                        seed,
                    ))
                }
            };

            let driver = BotDriver {
                my_id: player_id,
                my_name: bot_name.clone(),
                lobby_id: lobby_id.clone(),
                bot,
                thinking_time: thinking_time.clone(),
                lobby_sender: self.event_tx.clone(),
                receiver: bot_rx,
                id_to_name: id_to_name.clone(),
                name_to_id: name_to_id.clone(),
            };
            tokio::spawn(driver.run());
        }

        let session = GameSession { game, id_to_name: id_to_name.clone(), name_to_id: name_to_id.clone(), participants };

        // Transition lobby to InGame
        if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
            lobby.state = LobbyState::InGame(session);
        }

        // Transition human players to InGame phase
        for (addr, _) in &human_slots {
            if let Some(record) = self.players.get_mut(addr) {
                record.phase = ClientPhase::InGame { lobby_id: lobby_id.clone() };
            }
        }

        // Send GameStarted to all participants
        if let Some(lobby) = self.lobbies.get(&lobby_id)
            && let LobbyState::InGame(session) = &lobby.state {
                session.broadcast(ServerMessage::GameStarted).await;
            }

        // Build data for initial snapshots
        let (participant_names, game_players, inactive_players, current_player_name, deck_size) = {
            let lobby = match self.lobbies.get(&lobby_id) {
                Some(l) => l,
                None => return,
            };
            let session = match &lobby.state {
                LobbyState::InGame(s) => s,
                _ => return,
            };
            let names: Vec<String> = session.participants.iter().map(|p| p.name.clone()).collect();
            let current_id = session.game.get_current_player().map(|p| p.id);
            let current_name = current_id
                .and_then(|id| session.id_to_name.get(&id))
                .cloned()
                .unwrap_or_default();
            let deck = session.game.deck.len();
            (names, session.game.players.clone(), session.game.inactive_players.clone(), current_name, deck)
        };

        // Send personalised initial GameSnapshot to each participant
        self.broadcast_snapshots(
            &lobby_id,
            &participant_names,
            &game_players,
            &inactive_players,
            &current_player_name,
            deck_size,
            None,
        ).await;

        info!(lobby_id = %lobby_id, player_count = %total, "game session started");
    }

    async fn send(&self, address: SocketAddr, message: ServerMessage) {
        if let Some(record) = self.players.get(&address)
            && record.sender.send(message).await.is_err() {
                tracing::warn!(%address, "failed to send message — client channel closed");
            }
    }
}

// ── BotDriver ─────────────────────────────────────────────────────────────────

struct BotDriver {
    my_id: go_fish::PlayerId,
    my_name: String,
    lobby_id: String,
    bot: Box<dyn Bot + Send>,
    thinking_time: ThinkingTimeConfig,
    lobby_sender: mpsc::Sender<LobbyEvent>,
    receiver: mpsc::Receiver<ServerMessage>,
    id_to_name: HashMap<go_fish::PlayerId, String>,
    name_to_id: HashMap<String, go_fish::PlayerId>,
}

impl BotDriver {
    async fn run(mut self) {
        while let Some(msg) = self.receiver.recv().await {
            match msg {
                ServerMessage::GameResult(_) | ServerMessage::GameAborted => {
                    return;
                }
                ServerMessage::GameSnapshot(snapshot) => {
                    // Collect valid targets (all other participants)
                    let valid_targets: Vec<go_fish::PlayerId> = snapshot.opponents.iter()
                        .filter_map(|o| self.name_to_id.get(&o.name).copied())
                        .collect();

                    // Build and submit observation
                    let last_hook_outcome = snapshot.last_hook_outcome.as_ref().and_then(|o| {
                        let fisher = *self.name_to_id.get(&o.fisher_name)?;
                        let target = *self.name_to_id.get(&o.target_name)?;
                        Some(go_fish::HookOutcome { fisher, target, rank: o.rank, result: o.result.clone() })
                    });
                    let observation = BotObservation {
                        my_hand: snapshot.hand_state.hand.books.clone(),
                        my_completed_books: snapshot.hand_state.completed_books.clone(),
                        opponents: snapshot.opponents.iter()
                            .filter_map(|o| {
                                let id = *self.name_to_id.get(&o.name)?;
                                Some(OpponentView {
                                    id,
                                    hand_size: o.card_count,
                                    completed_books: o.completed_books.clone(),
                                })
                            })
                            .collect(),
                        deck_size: snapshot.deck_size,
                        active_player_id: *self.name_to_id.get(&snapshot.active_player)
                            .unwrap_or(&self.my_id),
                        last_hook_outcome,
                    };
                    self.bot.observe(observation);

                    // Only act when it's our turn
                    if snapshot.active_player != self.my_name {
                        continue;
                    }

                    if valid_targets.is_empty() {
                        continue;
                    }

                    // Simulate thinking time
                    let delay_ms = if self.thinking_time.max_ms > self.thinking_time.min_ms {
                        use rand::RngExt as _;
                        rand::rng().random_range(self.thinking_time.min_ms..=self.thinking_time.max_ms)
                    } else {
                        self.thinking_time.min_ms
                    };
                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }

                    // Generate hook
                    let hook = self.bot.generate_hook(&valid_targets);
                    let target_name = match self.id_to_name.get(&hook.target) {
                        Some(n) => n.clone(),
                        None => continue,
                    };

                    let _ = self.lobby_sender.send(LobbyEvent::Hook {
                        lobby_id: self.lobby_id.clone(),
                        player_name: self.my_name.clone(),
                        request: go_fish_web::ClientHookRequest {
                            target_name,
                            rank: hook.rank,
                        },
                    }).await;
                }
                _ => {}
            }
        }
    }
}

/// Generate a random lobby name as an adjective-water-place combo (e.g. `murky-river`).
pub fn random_water_name() -> String {
    use rand::seq::IndexedRandom;
    const ADJECTIVES: &[&str] = &[
        "murky", "crystal", "rushing", "still", "shallow",
        "deep", "misty", "cold", "tidal", "glassy",
        "brackish", "rocky", "turbid", "frosty", "ancient",
        "hidden", "winding", "sunlit", "placid", "mossy",
    ];
    const PLACES: &[&str] = &[
        "river", "ocean", "creek", "sea", "lake",
        "pond", "stream", "brook", "bay", "cove",
        "estuary", "lagoon", "marsh", "reef", "fjord",
        "inlet", "delta", "pool", "tributary", "shoal",
    ];
    let mut rng = rand::rng();
    let adj = ADJECTIVES.choose(&mut rng).unwrap();
    let place = PLACES.choose(&mut rng).unwrap();
    format!("{}-{}", adj, place)
}

/// Generate a random player name as an adjective-fish combo (e.g. `dreamy-bream`).
pub fn random_fish_name() -> String {
    use rand::seq::IndexedRandom;
    const ADJECTIVES: &[&str] = &[
        "dreamy", "glistening", "silvery", "slippery", "scaly",
        "darting", "speckled", "mossy", "murky", "shimmery",
        "dappled", "glinting", "swift", "sleepy", "bubbly",
        "placid", "sunlit", "drifting", "whiskered", "winding",
    ];
    const FISH: &[&str] = &[
        "bream", "trout", "bass", "carp", "perch",
        "pike", "roach", "dace", "tench", "catfish",
        "chub", "gudgeon", "salmon", "minnow", "rudd",
        "haddock", "mullet", "flounder", "plaice", "cod",
    ];
    let mut rng = rand::rng();
    let adj = ADJECTIVES.choose(&mut rng).unwrap();
    let fish = FISH.choose(&mut rng).unwrap();
    format!("{}-{}", adj, fish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use go_fish_web::ClientMessage;
    use proptest::prelude::*;
    use std::net::SocketAddr;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    /// Test-local outbound message (replaces the removed LobbyOutboundMessage).
    #[derive(Debug)]
    struct TestOutboundMessage {
        address: SocketAddr,
        message: ServerMessage,
    }

    fn make_lobby_manager(max_players: usize) -> (
        mpsc::Sender<LobbyEvent>,
        mpsc::Sender<LobbyCommand>,
        tokio::task::JoinHandle<()>,
    ) {
        let (event_tx, event_rx) = mpsc::channel::<LobbyEvent>(64);
        let (cmd_tx, cmd_rx) = mpsc::channel::<LobbyCommand>(8);
        let bot_config = BotConfig {
            thinking_time_min_ms: 0,
            thinking_time_max_ms: 0,
            simple_bot: SimpleBotConfig { memory_limit: 5, error_margin: 0.15 },
        };
        let manager = LobbyManager::new(event_rx, cmd_rx, max_players, event_tx.clone(), bot_config);
        let handle = tokio::spawn(manager.run());
        (event_tx, cmd_tx, handle)
    }

    fn make_shared_channel() -> (
        mpsc::Sender<TestOutboundMessage>,
        mpsc::Receiver<TestOutboundMessage>,
    ) {
        mpsc::channel(128)
    }

    fn addr(n: u16) -> SocketAddr {
        format!("127.0.0.1:{}", 20000 + n).parse().unwrap()
    }

    /// Connect a client and forward all its messages to a shared outbound channel.
    async fn connect_client(
        event_tx: &mpsc::Sender<LobbyEvent>,
        shared_tx: &mpsc::Sender<TestOutboundMessage>,
        address: SocketAddr,
    ) {
        let (web_tx, mut web_rx) = mpsc::channel::<ServerMessage>(64);
        let tx = shared_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = web_rx.recv().await {
                let _ = tx.send(TestOutboundMessage { address, message: msg }).await;
            }
        });
        event_tx.send(LobbyEvent::ClientConnected { address, message_tx: web_tx }).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: Identity assigns name and sends PlayerIdentity
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn identity_assigns_name_and_sends_player_identity() {
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let address = addr(1);

        connect_client(&event_tx, &outbound_tx, address).await;
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
            let parts: Vec<&str> = name.splitn(2, '-').collect();
            assert_eq!(parts.len(), 2, "name should contain a hyphen: {}", name);
            assert!(!parts[0].is_empty(), "adjective part should not be empty");
            assert!(!parts[1].is_empty(), "fish part should not be empty");
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let address = addr(2);

        connect_client(&event_tx, &outbound_tx, address).await;
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let address = addr(3);

        // First: connect and identify
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, address).await;

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
                let (event_tx, cmd_tx, _handle) = make_lobby_manager(16);
                let (outbound_tx, mut outbound_rx) = make_shared_channel();

                // Generate N distinct socket addresses
                let addresses: Vec<SocketAddr> = (0..n)
                    .map(|i| format!("127.0.0.2:{}", 30000 + i as u16).parse().unwrap())
                    .collect();

                // Send ClientConnected + Identity for each
                for &address in &addresses {
                    connect_client(&event_tx, &outbound_tx, address).await;
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
        outbound_tx: &mpsc::Sender<TestOutboundMessage>,
        outbound_rx: &mut mpsc::Receiver<TestOutboundMessage>,
        address: SocketAddr,
    ) -> String {
        connect_client(event_tx, outbound_tx, address).await;
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let address = addr(10);

        let name = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, address).await;

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
                assert!(lobby_id.contains('-'), "lobby_id should contain a hyphen: {}", lobby_id);
                assert_eq!(leader, name);
                assert_eq!(players, vec![LobbyPlayer::Human { name: name.clone() }]);
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(20);
        let addr_b = addr(21);

        // Player A creates lobby
        let name_a = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        let name_b = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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
                    assert!(players.iter().any(|p| p.name() == name_a));
                    assert!(players.iter().any(|p| p.name() == name_b));
                    assert_eq!(*max_players, 4);
                    got_lobby_joined_b = true;
                }
                (a, ServerMessage::LobbyUpdated { leader, players })
                    if a == addr_a =>
                {
                    assert_eq!(leader, &name_a);
                    assert!(players.iter().any(|p| p.name() == name_a));
                    assert!(players.iter().any(|p| p.name() == name_b));
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let address = addr(30);

        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, address).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(40);
        let addr_b = addr(41);
        let addr_c = addr(42);

        // A creates lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();
        // Drain LobbyJoined (B), LobbyUpdated (A), GameStarted*2, GameSnapshot*2 = 6 total
        for _ in 0..6 {
            timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out").expect("channel closed");
        }

        // C tries to join the full lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_c).await;
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(50);
        let addr_b = addr(51);

        // A creates lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Collect all messages: LobbyJoined(B), LobbyUpdated(A), GameStarted*2, GameSnapshot*2 = 6
        let mut got_lobby_joined = false;
        for _ in 0..6 {
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
                let (event_tx, cmd_tx, _handle) = make_lobby_manager(max_players);
                let (outbound_tx, mut outbound_rx) = make_shared_channel();
                let address: SocketAddr = "127.0.0.3:40000".parse().unwrap();

                let name = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, address).await;

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
                        prop_assert!(lobby_id.contains('-'), "lobby_id should contain a hyphen: {}", lobby_id);
                        prop_assert_eq!(&leader, &name, "leader should be the creating player");
                        prop_assert_eq!(players, vec![LobbyPlayer::Human { name: name.clone() }], "players list should contain only the creator");
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
                let (event_tx, cmd_tx, _handle) = make_lobby_manager(max_players);
                let (outbound_tx, mut outbound_rx) = make_shared_channel();

                // Connect and identify all players
                let addresses: Vec<SocketAddr> = (0..n)
                    .map(|i| format!("127.0.0.4:{}", 50000 + i as u16).parse().unwrap())
                    .collect();

                let mut names = Vec::new();
                for &address in &addresses {
                    let name = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, address).await;
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
                                // No duplicate names in players list
                                let unique: std::collections::HashSet<String> = players.iter().map(|p| p.name().to_string()).collect();
                                prop_assert_eq!(unique.len(), players.len(),
                                    "duplicate players in LobbyJoined: {:?}", players);
                                player_counts_in_updates.push(players.len());
                                leaders_seen.push(leader.clone());
                            }
                            ServerMessage::LobbyUpdated { players, leader } => {
                                // Player count must not exceed max_players
                                prop_assert!(players.len() <= max_players,
                                    "players.len() {} > max_players {}", players.len(), max_players);
                                // No duplicate names
                                let unique: std::collections::HashSet<String> = players.iter().map(|p| p.name().to_string()).collect();
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(100);
        let addr_b = addr(101);

        // A creates lobby
        let name_a = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        let _name_b = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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
                assert_eq!(players, vec![LobbyPlayer::Human { name: name_a.clone() }]);
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(110);
        let addr_b = addr(111);

        // A creates lobby
        let _name_a = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        let name_b = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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
                assert_eq!(players, vec![LobbyPlayer::Human { name: name_b.clone() }]);
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(120);

        // A creates lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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

        // LeaveLobby should be sent, but not LobbyUpdate
        let msg = timeout(Duration::from_millis(200), outbound_rx.recv())
            .await.expect("timed out").expect("channel closed");
        match msg.message {
            ServerMessage::LobbyLeft(LobbyLeftReason::RequestedByPlayer) => {},
            other => panic!("expected LobbyLeft, got {:?}", other),
        }

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(130);

        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(140);
        let addr_b = addr(141);

        // A creates lobby
        let name_a = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        let _name_b = connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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
                assert_eq!(players, vec![LobbyPlayer::Human { name: name_a.clone() }]);
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(150);

        // Connect but don't identify
        connect_client(&event_tx, &outbound_tx, addr_a).await;

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
                let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
                let (outbound_tx, mut outbound_rx) = make_shared_channel();

                let disconnecting_addr: SocketAddr = "127.0.0.5:60000".parse().unwrap();
                let other_addr: SocketAddr = "127.0.0.5:60001".parse().unwrap();

                match state {
                    0 => {
                        // Negotiating: connect but don't identify
                        connect_client(&event_tx, &outbound_tx, disconnecting_addr).await;
                    }
                    1 => {
                        // Pre-lobby: connect and identify
                        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, disconnecting_addr).await;
                    }
                    2 => {
                        // In-lobby: connect, identify, create lobby, have another player join
                        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, disconnecting_addr).await;
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
                        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, other_addr).await;
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

                let mut messages: Vec<TestOutboundMessage> = Vec::new();
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(200);
        let addr_b = addr(201);

        // A creates lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(210);

        // A creates lobby (alone)
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(4);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(220);
        let addr_b = addr(221);

        // A creates lobby
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
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

        // Collect 4 messages: for each of A and B: GameStarted, GameSnapshot
        let mut msgs: Vec<TestOutboundMessage> = Vec::new();
        for _ in 0..4 {
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
            assert_eq!(player_msgs.len(), 2, "each player should receive 2 messages");
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::GameStarted)),
                "player {:?} should receive GameStarted", player_addr);
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::GameSnapshot(_))),
                    "player {:?} should receive GameSnapshot", player_addr);
        }

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: auto-start when lobby full starts game (sends GameStarted to all)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn auto_start_when_lobby_full_starts_game() {
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(230);
        let addr_b = addr(231);

        // A creates lobby (max_players=2)
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Collect all messages: LobbyJoined(B), LobbyUpdated(A), GameStarted*2, GameSnapshot*2 = 6
        let mut msgs: Vec<TestOutboundMessage> = Vec::new();
        for _ in 0..6 {
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
        outbound_tx: &mpsc::Sender<TestOutboundMessage>,
        outbound_rx: &mut mpsc::Receiver<TestOutboundMessage>,
        addr_a: SocketAddr,
        addr_b: SocketAddr,
    ) -> (String, String, SocketAddr, go_fish_web::HandState, go_fish_web::HandState) {
        // A creates lobby
        let name_a = connect_and_identify(event_tx, outbound_tx, outbound_rx, addr_a).await;
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
        let name_b = connect_and_identify(event_tx, outbound_tx, outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id),
        }).await.unwrap();

        // Drain all startup messages: LobbyJoined(B), LobbyUpdated(A), GameStarted*2, GameSnapshot*2 = 6
        let mut whose_turn: Option<SocketAddr> = None;
        let mut hand_state_a: Option<go_fish_web::HandState> = None;
        let mut hand_state_b: Option<go_fish_web::HandState> = None;
        for _ in 0..6 {
            let msg = timeout(Duration::from_secs(2), outbound_rx.recv())
                .await.expect("timed out draining startup").expect("channel closed");
            match &msg.message {
                ServerMessage::GameSnapshot(snapshot) if msg.address == addr_a => {
                    if whose_turn.is_none() && snapshot.active_player == name_a {
                        whose_turn = Some(addr_a);
                    } else if whose_turn.is_none() {
                        whose_turn = Some(addr_b);
                    }
                    hand_state_a = Some(snapshot.hand_state.clone());
                }
                ServerMessage::GameSnapshot(snapshot) if msg.address == addr_b => {
                    if whose_turn.is_none() && snapshot.active_player == name_b {
                        whose_turn = Some(addr_b);
                    } else if whose_turn.is_none() {
                        whose_turn = Some(addr_a);
                    }
                    hand_state_b = Some(snapshot.hand_state.clone());
                }
                _ => {}
            }
        }

        let whose_turn = whose_turn.expect("should have determined whose turn it is");
        let hand_state_a = hand_state_a.expect("should have received GameSnapshot for A");
        let hand_state_b = hand_state_b.expect("should have received GameSnapshot for B");

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(300);
        let addr_b = addr(301);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(310);
        let addr_b = addr(311);

        let (_name_a, _name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(320);
        let addr_b = addr(321);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(330);
        let addr_b = addr(331);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(340);
        let addr_b = addr(341);

        let (name_a, name_b, whose_turn, hand_state_a, hand_state_b) =
            setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

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

        // Collect messages: 1 GameSnapshot per player = 2 messages
        let mut msgs: Vec<TestOutboundMessage> = Vec::new();
        for _ in 0..2 {
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
            assert_eq!(player_msgs.len(), 1, "each player should receive 1 message after hook");
            assert!(player_msgs.iter().any(|m| matches!(m, ServerMessage::GameSnapshot(_))),
                    "player {:?} should receive GameSnapshot", player_addr);
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
                let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
                let (outbound_tx, mut outbound_rx) = make_shared_channel();

                // Lobby 1: players at ports 11000-11001
                let addr_l1_a: SocketAddr = "127.0.2.1:11000".parse().unwrap();
                let addr_l1_b: SocketAddr = "127.0.2.1:11001".parse().unwrap();
                // Lobby 2: players at ports 11002-11003
                let addr_l2_a: SocketAddr = "127.0.2.1:11002".parse().unwrap();
                let addr_l2_b: SocketAddr = "127.0.2.1:11003".parse().unwrap();

                // Start lobby 1
                let (name_l1_a, name_l1_b, whose_turn_l1, hand_l1_a, hand_l1_b) =
                    setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_l1_a, addr_l1_b).await;

                // Start lobby 2
                let (_name_l2_a, _name_l2_b, _whose_turn_l2, _hand_l2_a, _hand_l2_b) =
                    setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_l2_a, addr_l2_b).await;

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
                let mut messages: Vec<TestOutboundMessage> = Vec::new();
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

                // Assert: lobby 1 players DO receive messages (GameSnapshot)
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(240);
        let addr_b = addr(241);

        // A creates lobby (max_players=2)
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_a).await;
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
        connect_and_identify(&event_tx, &outbound_tx, &mut outbound_rx, addr_b).await;
        event_tx.send(LobbyEvent::ClientMessage {
            address: addr_b,
            message: ClientMessage::JoinLobby(lobby_id.clone()),
        }).await.unwrap();

        // Drain all auto-start messages (2 lobby + 4 game = 6)
        for _ in 0..6 {
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
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(400);
        let addr_b = addr(401);

        // Start a 2-player game
        setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

        // Player B disconnects
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_b,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // Collect messages — player A should receive GameAborted, player B should NOT
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let mut messages: Vec<TestOutboundMessage> = Vec::new();
        while let Ok(Some(msg)) = timeout(Duration::from_millis(100), outbound_rx.recv()).await {
            messages.push(msg);
        }

        // Player A should receive GameAborted
        let a_got_aborted = messages.iter().any(|m| {
            m.address == addr_a && matches!(m.message, ServerMessage::GameAborted)
        });
        assert!(a_got_aborted, "player A should receive GameAborted after B disconnects");

        // Player B should NOT receive GameAborted (they disconnected)
        let b_got_aborted = messages.iter().any(|m| {
            m.address == addr_b && matches!(m.message, ServerMessage::GameAborted)
        });
        assert!(!b_got_aborted, "player B should NOT receive GameAborted after disconnecting");

        cmd_tx.send(LobbyCommand::Shutdown).await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Test: after game ends, players can create a new lobby (proves PreLobby state)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn after_game_ends_players_can_create_new_lobby() {
        let (event_tx, cmd_tx, _handle) = make_lobby_manager(2);
        let (outbound_tx, mut outbound_rx) = make_shared_channel();
        let addr_a = addr(410);
        let addr_b = addr(411);

        // Start a 2-player game
        setup_two_player_game_with_state(&event_tx, &outbound_tx, &mut outbound_rx, addr_a, addr_b).await;

        // Player B disconnects (ends the game)
        event_tx.send(LobbyEvent::ClientDisconnected {
            address: addr_b,
            reason: crate::connection::DisconnectReason::Clean,
        }).await.unwrap();

        // Drain the GameAborted message sent to player A
        let aborted_msg = timeout(Duration::from_secs(2), outbound_rx.recv())
            .await.expect("timed out waiting for GameAborted").expect("channel closed");
        assert!(
            matches!(aborted_msg.message, ServerMessage::GameAborted),
            "expected GameAborted, got {:?}", aborted_msg.message
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
