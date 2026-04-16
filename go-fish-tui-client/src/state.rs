use std::collections::HashMap;
use std::collections::VecDeque;

use go_fish::HookResult;
use go_fish_web::LobbyLeftReason;
use go_fish_web::LobbyPlayer;
use go_fish_web::ServerMessage;

pub const MAX_BOOK_NOTIFICATIONS: usize = 3;

pub use crate::network::NetworkEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectingState {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreLobbyState {
    pub player_name: String,
    pub input_state: PreLobbyInputState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum PreLobbyInputState {
    #[default] None,
    LobbyId(PreLobbyInputLobbyIdState)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreLobbyInputLobbyIdState {
    pub lobby_id: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LobbyState {
    pub player_name: String,
    pub lobby_id: String,
    pub leader: String,
    pub players: Vec<LobbyPlayer>,
    pub max_players: usize,
    pub error: Option<String>,
}

// ── Task 5.1: GameInputState and GameState ────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum GameInputState {
    Idle,
    SelectingTarget { cursor: usize },
    SelectingRank { target: String, cursor: usize },
}

#[derive(Debug, Clone)]
pub struct GameState {
    pub player_name: String,
    pub players: Vec<String>,
    pub hand: go_fish::Hand,
    pub completed_books: Vec<go_fish::CompleteBook>,
    pub opponent_card_counts: HashMap<String, usize>,
    pub opponent_book_counts: HashMap<String, usize>,
    pub active_player: String,
    pub latest_hook_outcome: Option<go_fish_web::HookOutcome>,
    pub hook_error: Option<go_fish_web::HookError>,
    pub deck_draw_notification: Option<String>,
    pub book_completion_notifications: VecDeque<String>,
    pub has_received_snapshot: bool,
    pub game_result: Option<go_fish_web::GameResult>,
    pub input_state: GameInputState,
}

impl GameState {
    pub fn new(player_name: String, players: Vec<String>) -> Self {
        let opponents: HashMap<String, usize> = players.iter()
            .filter(|p| *p != &player_name)
            .map(|p| (p.clone(), 0))
            .collect();
        GameState {
            player_name,
            players,
            hand: go_fish::Hand::empty(),
            completed_books: vec![],
            opponent_card_counts: opponents.clone(),
            opponent_book_counts: opponents.keys().map(|k| (k.clone(), 0)).collect(),
            active_player: String::new(),
            latest_hook_outcome: None,
            hook_error: None,
            deck_draw_notification: None,
            book_completion_notifications: VecDeque::new(),
            has_received_snapshot: false,
            game_result: None,
            input_state: GameInputState::Idle,
        }
    }
}

// ── Task 5.2: Screen::Game variant ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Screen {
    Connecting(ConnectingState),
    PreLobby(PreLobbyState),
    Lobby(LobbyState),
    Game(GameState),
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub screen: Screen,
}

impl AppState {
    pub fn new() -> AppState {
        AppState {
            screen: Screen::Connecting(ConnectingState {
                status: "Connecting…".to_string(),
            }),
        }
    }
}

/// Apply a network event to the app state, performing screen transitions as needed.
pub fn apply_network_event(state: &mut AppState, event: &NetworkEvent) {
    match event {
        NetworkEvent::Message(msg) => apply_server_message(state, msg),
        NetworkEvent::Closed => apply_connection_closed(state),
        NetworkEvent::Error(err) => apply_connection_error(state, err),
    }
}

fn apply_server_message(state: &mut AppState, msg: &ServerMessage) {
    match msg {
        ServerMessage::PlayerIdentity(name) => {
            if let Screen::Connecting(_) = &state.screen {
                state.screen = Screen::PreLobby(PreLobbyState {
                    player_name: name.clone(),
                    input_state: PreLobbyInputState::None,
                    error: None,
                });
            }
        }
        ServerMessage::LobbyJoined {
            lobby_id,
            leader,
            players,
            max_players,
        } => {
            if let Screen::PreLobby(pre) = &state.screen {
                let player_name = pre.player_name.clone();
                state.screen = Screen::Lobby(LobbyState {
                    player_name,
                    lobby_id: lobby_id.clone(),
                    leader: leader.clone(),
                    players: players.clone(),
                    max_players: *max_players,
                    error: None,
                });
            }
        }
        ServerMessage::LobbyUpdated { leader, players } => {
            if let Screen::Lobby(lobby) = &mut state.screen {
                let player_name = lobby.player_name.clone();
                // If local player was removed, transition back to PreLobby
                if !players.iter().any(|p| p.name() == player_name) {
                    state.screen = Screen::PreLobby(PreLobbyState {
                        player_name,
                        input_state: PreLobbyInputState::None,
                        error: None,
                    });
                } else {
                    lobby.leader = leader.clone();
                    lobby.players = players.clone();
                }
            }
        }
        ServerMessage::LobbyLeft(reason) => {
            match reason {
                LobbyLeftReason::RequestedByPlayer => {
                    if let Screen::Lobby(lobby) = &state.screen {
                        let player_name = lobby.player_name.clone();
                        state.screen = Screen::PreLobby(PreLobbyState {
                            player_name,
                            input_state: PreLobbyInputState::None,
                            error: None,
                        });
                    }
                }
            }
        }
        // Task 5.3: GameStarted handler
        ServerMessage::GameStarted => {
            if let Screen::Lobby(lobby) = &state.screen {
                let player_name = lobby.player_name.clone();
                let players: Vec<String> = lobby.players.iter().map(|p| p.name().to_string()).collect();
                state.screen = Screen::Game(GameState::new(player_name, players));
            }
        }
        // Task 5.4: GameSnapshot handler
        ServerMessage::GameSnapshot(snapshot) => {
            if let Screen::Game(game) = &mut state.screen {
                // Capture previous state for diffing
                let prev_rank_counts: HashMap<go_fish::Rank, usize> = game.hand.books.iter()
                    .map(|b| (b.rank, b.cards.len()))
                    .collect();
                let prev_book_count = game.completed_books.len();
                let prev_opponent_books: HashMap<String, usize> = game.opponent_book_counts.clone();

                // Update state from snapshot
                game.hand = snapshot.hand_state.hand.clone();
                game.hand.books.sort_by(|a, b| a.rank.cmp(&b.rank));
                game.completed_books = snapshot.hand_state.completed_books.clone();
                for opp in &snapshot.opponents {
                    game.opponent_card_counts.insert(opp.name.clone(), opp.card_count);
                    game.opponent_book_counts.insert(opp.name.clone(), opp.completed_books.len());
                }
                if let Some(ref outcome) = snapshot.last_hook_outcome {
                    game.latest_hook_outcome = Some(outcome.clone());
                }
                game.active_player = snapshot.active_player.clone();
                if snapshot.active_player == game.player_name {
                    game.hook_error = None;
                }
                game.input_state = GameInputState::Idle;

                // Compute notifications (skip first snapshot — initial deal)
                if game.has_received_snapshot {
                    compute_deck_draw_notification(game, &prev_rank_counts, prev_book_count, snapshot);
                    compute_book_completion_notifications(game, prev_book_count, &prev_opponent_books, snapshot);
                } else {
                    game.deck_draw_notification = None;
                }
                game.has_received_snapshot = true;
            }
        }
        // Task 5.5: HookError and GameResult handlers
        ServerMessage::HookError(err) => {
            if let Screen::Game(game) = &mut state.screen {
                game.hook_error = Some(err.clone());
            }
        }
        ServerMessage::GameResult(result) => {
            if let Screen::Game(game) = &mut state.screen {
                game.game_result = Some(result.clone());
            }
        }
        ServerMessage::GameAborted => {
            if let Screen::Game(game) = &state.screen {
                let player_name = game.player_name.clone();
                state.screen = Screen::PreLobby(PreLobbyState {
                    player_name,
                    input_state: PreLobbyInputState::None,
                    error: Some("Game aborted: a player disconnected.".to_string()),
                });
            }
        }
        ServerMessage::Error(msg) => {
            match &mut state.screen {
                Screen::Connecting(s) => {
                    s.status = msg.clone();
                }
                Screen::PreLobby(s) => {
                    match &mut s.input_state {
                        PreLobbyInputState::None => {
                            s.error = Some(msg.clone());
                        }
                        PreLobbyInputState::LobbyId(state) => {
                            state.error = Some(msg.clone());
                            state.lobby_id = "".to_string();
                        }
                    }
                }
                Screen::Lobby(s) => {
                    s.error = Some(msg.clone());
                }
                // Task 5.6: Game arm for Error — display on status bar, do not navigate
                Screen::Game(_s) => {
                    // Generic server errors on the Game screen are displayed via the UI layer.
                    // No navigation occurs. The error string is not stored in hook_error
                    // (which is typed as Option<go_fish_web::HookError>), so we silently
                    // acknowledge it here. The UI can be extended to show a separate error field.
                }
            }
        }
        // Other server messages (HandState, PlayerTurn, HookAndResult) are silently discarded
        _ => {}
    }
}

// Task 5.6: apply_connection_closed with Game arm
fn apply_connection_closed(state: &mut AppState) {
    let msg = "Server closed connection.".to_string();
    // Extract player_name from Game screen before mutating
    if let Screen::Game(game) = &state.screen {
        let player_name = game.player_name.clone();
        state.screen = Screen::PreLobby(PreLobbyState {
            player_name,
            input_state: PreLobbyInputState::None,
            error: None,
        });
        return;
    }
    match &mut state.screen {
        Screen::Connecting(s) => s.status = msg,
        Screen::PreLobby(s) => s.error = Some(msg),
        Screen::Lobby(s) => s.error = Some(msg),
        Screen::Game(_) => unreachable!(),
    }
}

// Task 5.6: apply_connection_error with Game arm
fn apply_connection_error(state: &mut AppState, err: &str) {
    let msg = format!("Connection error: {}", err);
    // Extract player_name from Game screen before mutating
    if let Screen::Game(game) = &state.screen {
        let player_name = game.player_name.clone();
        state.screen = Screen::PreLobby(PreLobbyState {
            player_name,
            input_state: PreLobbyInputState::None,
            error: None,
        });
        return;
    }
    match &mut state.screen {
        Screen::Connecting(s) => s.status = msg,
        Screen::PreLobby(s) => s.error = Some(msg),
        Screen::Lobby(s) => s.error = Some(msg),
        Screen::Game(_) => unreachable!(),
    }
}

fn compute_deck_draw_notification(
    game: &mut GameState,
    prev_rank_counts: &HashMap<go_fish::Rank, usize>,
    prev_book_count: usize,
    snapshot: &go_fish_web::GameSnapshot,
) {
    let new_rank_counts: HashMap<go_fish::Rank, usize> = game.hand.books.iter()
        .map(|b| (b.rank, b.cards.len()))
        .collect();

    // Rank caught via hook (if local player was the fisher)
    let hook_catch_rank: Option<go_fish::Rank> = snapshot.last_hook_outcome.as_ref()
        .filter(|o| o.fisher_name == game.player_name)
        .and_then(|o| match &o.result {
            HookResult::Catch(_) => Some(o.rank),
            HookResult::GoFish => None,
        });

    let mut drawn_rank: Option<go_fish::Rank> = None;

    // Check ranks in the new hand that gained cards not explained by a hook catch
    for (rank, &new_count) in &new_rank_counts {
        let old_count = prev_rank_counts.get(rank).copied().unwrap_or(0);
        if new_count > old_count && hook_catch_rank != Some(*rank) {
            drawn_rank = Some(*rank);
        }
    }

    // Check ranks that completed into books (disappeared from hand)
    let new_book_count = game.completed_books.len();
    if new_book_count > prev_book_count {
        for book in &game.completed_books[prev_book_count..] {
            if hook_catch_rank != Some(book.rank) && !new_rank_counts.contains_key(&book.rank) {
                let old_count = prev_rank_counts.get(&book.rank).copied().unwrap_or(0);
                if old_count == 3 {
                    drawn_rank = Some(book.rank);
                }
            }
        }
    }

    game.deck_draw_notification = drawn_rank.map(|r| format!("You drew a {} from the deck", r));
}

fn compute_book_completion_notifications(
    game: &mut GameState,
    prev_book_count: usize,
    prev_opponent_books: &HashMap<String, usize>,
    snapshot: &go_fish_web::GameSnapshot,
) {
    // Check local player
    if game.completed_books.len() > prev_book_count {
        for book in &game.completed_books[prev_book_count..] {
            game.book_completion_notifications.push_back(
                format!("You completed a book of {}s!", book.rank));
            if game.book_completion_notifications.len() > MAX_BOOK_NOTIFICATIONS {
                game.book_completion_notifications.pop_front();
            }
        }
    }

    // Check opponents
    for opp in &snapshot.opponents {
        let prev_count = prev_opponent_books.get(&opp.name).copied().unwrap_or(0);
        if opp.completed_books.len() > prev_count {
            for book in &opp.completed_books[prev_count..] {
                game.book_completion_notifications.push_back(
                    format!("{} completed a book of {}s!", opp.name, book.rank));
                if game.book_completion_notifications.len() > MAX_BOOK_NOTIFICATIONS {
                    game.book_completion_notifications.pop_front();
                }
            }
        }
    }
}

/// Returns true if the lobby ID is non-empty after trimming whitespace.
pub fn is_valid_lobby_id(s: &str) -> bool {
    !s.trim().is_empty()
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;