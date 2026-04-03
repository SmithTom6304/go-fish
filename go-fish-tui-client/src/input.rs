use tokio::sync::mpsc;

use go_fish_web::ClientMessage;

use crate::state::{
    AppState, GameInputState, PreLobbyInputLobbyIdState, PreLobbyInputState, Screen,
    is_valid_lobby_id,
};

// ── Platform-agnostic key types ───────────────────────────────────────────────

/// A platform-agnostic key code, covering the variants used by this app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Other,
}

/// A platform-agnostic key event.
pub struct KeyInput {
    pub key: Key,
    pub ctrl: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<crossterm::event::KeyEvent> for KeyInput {
    fn from(e: crossterm::event::KeyEvent) -> Self {
        use crossterm::event::{KeyCode, KeyModifiers};
        KeyInput {
            key: match e.code {
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Enter => Key::Enter,
                KeyCode::Backspace => Key::Backspace,
                KeyCode::Esc => Key::Esc,
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Left => Key::Left,
                KeyCode::Right => Key::Right,
                _ => Key::Other,
            },
            ctrl: e.modifiers.contains(KeyModifiers::CONTROL),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl From<ratzilla::event::KeyEvent> for KeyInput {
    fn from(e: ratzilla::event::KeyEvent) -> Self {
        use ratzilla::event::KeyCode;
        KeyInput {
            key: match e.code {
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Enter => Key::Enter,
                KeyCode::Backspace => Key::Backspace,
                KeyCode::Esc => Key::Esc,
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Left => Key::Left,
                KeyCode::Right => Key::Right,
                _ => Key::Other,
            },
            ctrl: e.ctrl,
        }
    }
}

// ── Key dispatch ──────────────────────────────────────────────────────────────

/// Handle a single key event.  Returns `true` if the application should quit.
///
/// Called by both the native event loop and the WASM key callback so all
/// key-dispatch logic lives in one place.
pub fn handle_key(
    state: &mut AppState,
    input: KeyInput,
    client_msg_tx: &mpsc::Sender<ClientMessage>,
) -> bool {
    if input.ctrl && input.key == Key::Char('c') {
        if let Screen::Lobby(_) = &state.screen {
            let _ = client_msg_tx.try_send(ClientMessage::LeaveLobby);
        }
        return true;
    }

    match &mut state.screen {
        Screen::PreLobby(pre) => {
            match &mut pre.input_state {
                PreLobbyInputState::None => {
                    if input.key == Key::Char('c') {
                        let _ = client_msg_tx.try_send(ClientMessage::CreateLobby);
                    } else if input.key == Key::Char('q') {
                        let _ = client_msg_tx.try_send(ClientMessage::LeaveLobby);
                        return true;
                    } else if input.key == Key::Char('j') {
                        pre.input_state = PreLobbyInputState::LobbyId(
                            PreLobbyInputLobbyIdState::default(),
                        );
                    }
                }
                PreLobbyInputState::LobbyId(lobby_id_state) => {
                    let lobby_id = &mut lobby_id_state.lobby_id;
                    match input.key {
                        Key::Char(ch) => {
                            lobby_id.push(ch);
                            lobby_id_state.error = None;
                        }
                        Key::Backspace => {
                            lobby_id.pop();
                        }
                        Key::Enter => {
                            if is_valid_lobby_id(lobby_id) {
                                let lobby_id = lobby_id.trim().to_string();
                                let _ = client_msg_tx
                                    .try_send(ClientMessage::JoinLobby(lobby_id));
                            } else {
                                lobby_id_state.error =
                                    Some("Please enter a valid Lobby ID".to_string());
                            }
                        }
                        Key::Esc => {
                            pre.input_state = PreLobbyInputState::None;
                        }
                        _ => {}
                    }
                }
            }
        }
        Screen::Lobby(lobby) => {
            if input.key == Key::Char('s')
                && lobby.players.len() >= 2
                && lobby.leader == lobby.player_name
            {
                let _ = client_msg_tx.try_send(ClientMessage::StartGame);
            } else if input.key == Key::Char('q') {
                let _ = client_msg_tx.try_send(ClientMessage::LeaveLobby);
            }
        }
        Screen::Connecting(_) => {}
        Screen::Game(game) => {
            if game.game_result.is_some() {
                if input.key == Key::Enter || input.key == Key::Char(' ') {
                    let player_name = game.player_name.clone();
                    state.screen = Screen::PreLobby(crate::state::PreLobbyState {
                        player_name,
                        input_state: PreLobbyInputState::None,
                        error: None,
                    });
                } else if input.key == Key::Char('q') {
                    return true;
                }
            } else {
                match &game.input_state {
                    GameInputState::Idle => {
                        if input.key == Key::Char('q') {
                            return true;
                        }
                        let opponents: Vec<String> = game
                            .players
                            .iter()
                            .filter(|p| *p != &game.player_name)
                            .cloned()
                            .collect();
                        if input.key == Key::Char('h')
                            && !opponents.is_empty()
                            && game.active_player == game.player_name
                        {
                            game.input_state = if opponents.len() == 1 {
                                GameInputState::SelectingRank {
                                    target: opponents[0].clone(),
                                    cursor: 0,
                                }
                            } else {
                                GameInputState::SelectingTarget { cursor: 0 }
                            };
                        }
                    }
                    GameInputState::SelectingTarget { cursor } => {
                        let opponents: Vec<String> = game
                            .players
                            .iter()
                            .filter(|p| *p != &game.player_name)
                            .cloned()
                            .collect();
                        let n = opponents.len();
                        if n == 0 {
                            return false;
                        }
                        let cursor = *cursor;
                        match input.key {
                            Key::Char('j') | Key::Down => {
                                game.input_state = GameInputState::SelectingTarget {
                                    cursor: (cursor + 1) % n,
                                };
                            }
                            Key::Char('k') | Key::Up => {
                                game.input_state = GameInputState::SelectingTarget {
                                    cursor: (cursor + n - 1) % n,
                                };
                            }
                            Key::Enter => {
                                if !game.hand.books.is_empty() {
                                    let target = opponents[cursor].clone();
                                    game.input_state =
                                        GameInputState::SelectingRank { target, cursor: 0 };
                                }
                            }
                            Key::Esc => {
                                game.input_state = GameInputState::Idle;
                            }
                            Key::Char('q') => {
                                return true;
                            }
                            _ => {}
                        }
                    }
                    GameInputState::SelectingRank { target, cursor } => {
                        let m = game.hand.books.len();
                        if m == 0 {
                            game.input_state = GameInputState::SelectingTarget { cursor: 0 };
                        } else {
                            let cursor = *cursor;
                            let target = target.clone();
                            match input.key {
                                Key::Char('l') | Key::Right => {
                                    game.input_state = GameInputState::SelectingRank {
                                        target,
                                        cursor: (cursor + 1) % m,
                                    };
                                }
                                Key::Char('h') | Key::Left => {
                                    game.input_state = GameInputState::SelectingRank {
                                        target,
                                        cursor: (cursor + m - 1) % m,
                                    };
                                }
                                Key::Enter => {
                                    let rank = game.hand.books[cursor].rank;
                                    let _ = client_msg_tx.try_send(ClientMessage::Hook(
                                        go_fish_web::ClientHookRequest {
                                            target_name: target,
                                            rank,
                                        },
                                    ));
                                    game.input_state = GameInputState::Idle;
                                }
                                Key::Esc => {
                                    game.input_state =
                                        GameInputState::SelectingTarget { cursor: 0 };
                                }
                                Key::Char('q') => {
                                    return true;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    false
}
