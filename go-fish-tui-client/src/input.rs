use tokio::sync::mpsc;

use go_fish_web::ClientMessage;

use crate::state::{
    AppState, BrowsingLobbiesState, BrowsingStatus, GameInputState,
    PreLobbyInputState, PreLobbyState, Screen,
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
            if input.key == Key::Char('c') {
                let _ = client_msg_tx.try_send(ClientMessage::CreateLobby);
            } else if input.key == Key::Char('q') {
                let _ = client_msg_tx.try_send(ClientMessage::LeaveLobby);
                return true;
            } else if input.key == Key::Char('j') {
                let player_name = pre.player_name.clone();
                state.screen = Screen::BrowsingLobbies(BrowsingLobbiesState {
                    player_name,
                    status: BrowsingStatus::Loading,
                    selected_index: 0,
                    frame_index: 0,
                });
                let _ = client_msg_tx.try_send(ClientMessage::RequestLobbies);
            }
        }
        Screen::Lobby(lobby) => {
            let is_leader = lobby.leader == lobby.player_name;
            if input.key == Key::Char('s')
                && lobby.players.len() >= 2
                && is_leader
            {
                let _ = client_msg_tx.try_send(ClientMessage::StartGame);
            } else if input.key == Key::Char('a') && is_leader {
                let _ = client_msg_tx.try_send(ClientMessage::AddBot {
                    bot_type: go_fish_web::BotType::SimpleBot,
                });
            } else if input.key == Key::Char('d') && is_leader {
                let _ = client_msg_tx.try_send(ClientMessage::RemoveBot);
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
        Screen::BrowsingLobbies(browsing) => {
            let is_creating = matches!(&browsing.status, BrowsingStatus::Creating);
            let is_entering_id = matches!(&browsing.status, BrowsingStatus::EnteringId { .. });

            // Esc/q: return to PreLobby (except when in EnteringId or Creating)
            if !is_creating && !is_entering_id
                && matches!(&input.key, Key::Esc | Key::Char('q'))
            {
                let player_name = browsing.player_name.clone();
                state.screen = Screen::PreLobby(PreLobbyState {
                    player_name,
                    input_state: PreLobbyInputState::None,
                    error: None,
                });
                return false;
            }

            if is_creating {
                return false;
            }

            if is_entering_id {
                // Defer mutations that change status to avoid double-borrow
                let mut go_loading = false;
                let mut join_id: Option<String> = None;
                if let BrowsingStatus::EnteringId { input: id_input, error } = &mut browsing.status {
                    *error = None;
                    match &input.key {
                        Key::Char(ch) => { id_input.push(*ch); }
                        Key::Backspace => { id_input.pop(); }
                        Key::Enter => {
                            let trimmed = id_input.trim().to_string();
                            if !trimmed.is_empty() {
                                join_id = Some(trimmed);
                            }
                        }
                        Key::Esc => { go_loading = true; }
                        _ => {}
                    }
                }
                if go_loading {
                    browsing.status = BrowsingStatus::Loading;
                    let _ = client_msg_tx.try_send(ClientMessage::RequestLobbies);
                }
                if let Some(id) = join_id {
                    let _ = client_msg_tx.try_send(ClientMessage::JoinLobby(id));
                }
                return false;
            }

            // Loading / Loaded / Error: cache list state before any mutations
            let loaded_len = if let BrowsingStatus::Loaded(l) = &browsing.status { l.len() } else { 0 };
            let enter_lobby_id = if let BrowsingStatus::Loaded(l) = &browsing.status {
                l.get(browsing.selected_index).map(|li| li.lobby_id.clone())
            } else {
                None
            };

            match input.key {
                Key::Char('c') => {
                    browsing.status = BrowsingStatus::Creating;
                    let _ = client_msg_tx.try_send(ClientMessage::CreateLobby);
                }
                Key::Char('r') => {
                    browsing.status = BrowsingStatus::Loading;
                    let _ = client_msg_tx.try_send(ClientMessage::RequestLobbies);
                }
                Key::Char('i') => {
                    browsing.status = BrowsingStatus::EnteringId {
                        input: String::new(),
                        error: None,
                    };
                }
                Key::Up | Key::Char('k') => {
                    if loaded_len > 0 {
                        browsing.selected_index =
                            (browsing.selected_index + loaded_len - 1) % loaded_len;
                    }
                }
                Key::Down | Key::Char('j') => {
                    if loaded_len > 0 {
                        browsing.selected_index = (browsing.selected_index + 1) % loaded_len;
                    }
                }
                Key::Enter => {
                    if let Some(id) = enter_lobby_id {
                        let _ = client_msg_tx.try_send(ClientMessage::JoinLobby(id));
                    }
                }
                _ => {}
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use go_fish_web::LobbyInfo;
    use tokio::sync::mpsc;

    fn make_tx() -> mpsc::Sender<ClientMessage> {
        mpsc::channel(8).0
    }

    fn key(k: Key) -> KeyInput {
        KeyInput { key: k, ctrl: false }
    }

    fn browsing_state(player_name: &str, status: BrowsingStatus) -> AppState {
        AppState {
            screen: Screen::BrowsingLobbies(BrowsingLobbiesState {
                player_name: player_name.to_string(),
                status,
                selected_index: 0,
                frame_index: 0,
            }),
        }
    }

    fn pre_lobby_state(player_name: &str) -> AppState {
        AppState {
            screen: Screen::PreLobby(PreLobbyState {
                player_name: player_name.to_string(),
                input_state: PreLobbyInputState::None,
                error: None,
            }),
        }
    }

    // ── PreLobby: j opens lobby browser ──────────────────────────────────────

    #[test]
    fn pre_lobby_j_transitions_to_browsing_lobbies() {
        let tx = make_tx();
        let mut state = pre_lobby_state("Alice");
        handle_key(&mut state, key(Key::Char('j')), &tx);
        assert!(matches!(state.screen, Screen::BrowsingLobbies(_)));
    }

    #[test]
    fn pre_lobby_j_sets_status_to_loading() {
        let tx = make_tx();
        let mut state = pre_lobby_state("Alice");
        handle_key(&mut state, key(Key::Char('j')), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(b.status, BrowsingStatus::Loading));
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    // ── BrowsingLobbies: Esc/q returns to PreLobby ───────────────────────────

    #[test]
    fn browsing_esc_returns_to_pre_lobby() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Loading);
        handle_key(&mut state, key(Key::Esc), &tx);
        assert!(matches!(state.screen, Screen::PreLobby(_)));
    }

    #[test]
    fn browsing_q_returns_to_pre_lobby() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(vec![]));
        handle_key(&mut state, key(Key::Char('q')), &tx);
        assert!(matches!(state.screen, Screen::PreLobby(_)));
    }

    // ── BrowsingLobbies: c transitions to Creating ───────────────────────────

    #[test]
    fn browsing_c_transitions_to_creating_from_loading() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Loading);
        handle_key(&mut state, key(Key::Char('c')), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(b.status, BrowsingStatus::Creating));
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    #[test]
    fn browsing_creating_all_keys_inert() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Creating);
        handle_key(&mut state, key(Key::Char('q')), &tx);
        // Should NOT have navigated away — Creating ignores Esc/q
        assert!(matches!(state.screen, Screen::BrowsingLobbies(_)));
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(b.status, BrowsingStatus::Creating));
        }
    }

    // ── BrowsingLobbies: r reloads ────────────────────────────────────────────

    #[test]
    fn browsing_r_resets_to_loading() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(vec![]));
        handle_key(&mut state, key(Key::Char('r')), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(b.status, BrowsingStatus::Loading));
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    // ── BrowsingLobbies: i enters id mode ────────────────────────────────────

    #[test]
    fn browsing_i_transitions_to_entering_id() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::Loading);
        handle_key(&mut state, key(Key::Char('i')), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(&b.status, BrowsingStatus::EnteringId { input, error } if input.is_empty() && error.is_none()));
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    // ── BrowsingLobbies: up/down navigation ──────────────────────────────────

    #[test]
    fn browsing_down_increments_selected_index_in_loaded() {
        let tx = make_tx();
        let lobbies = vec![
            LobbyInfo { lobby_id: "a".to_string(), player_count: 1, max_players: 4 },
            LobbyInfo { lobby_id: "b".to_string(), player_count: 1, max_players: 4 },
        ];
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(lobbies));
        handle_key(&mut state, key(Key::Down), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert_eq!(b.selected_index, 1);
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    #[test]
    fn browsing_down_wraps_to_first_from_last() {
        let tx = make_tx();
        let lobbies = vec![
            LobbyInfo { lobby_id: "a".to_string(), player_count: 1, max_players: 4 },
            LobbyInfo { lobby_id: "b".to_string(), player_count: 1, max_players: 4 },
        ];
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(lobbies));
        if let Screen::BrowsingLobbies(b) = &mut state.screen {
            b.selected_index = 1;
        }
        handle_key(&mut state, key(Key::Down), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert_eq!(b.selected_index, 0, "should wrap to first item");
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    #[test]
    fn browsing_up_decrements_selected_index() {
        let tx = make_tx();
        let lobbies = vec![
            LobbyInfo { lobby_id: "a".to_string(), player_count: 1, max_players: 4 },
            LobbyInfo { lobby_id: "b".to_string(), player_count: 1, max_players: 4 },
        ];
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(lobbies));
        if let Screen::BrowsingLobbies(b) = &mut state.screen {
            b.selected_index = 1;
        }
        handle_key(&mut state, key(Key::Up), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert_eq!(b.selected_index, 0);
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    #[test]
    fn browsing_up_wraps_to_last_from_first() {
        let tx = make_tx();
        let lobbies = vec![
            LobbyInfo { lobby_id: "a".to_string(), player_count: 1, max_players: 4 },
            LobbyInfo { lobby_id: "b".to_string(), player_count: 1, max_players: 4 },
            LobbyInfo { lobby_id: "c".to_string(), player_count: 1, max_players: 4 },
        ];
        let mut state = browsing_state("Alice", BrowsingStatus::Loaded(lobbies));
        handle_key(&mut state, key(Key::Up), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert_eq!(b.selected_index, 2, "should wrap to last item");
        } else {
            panic!("expected BrowsingLobbies");
        }
    }

    // ── EnteringId: char input, backspace, esc ────────────────────────────────

    #[test]
    fn entering_id_char_appends_to_input() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::EnteringId { input: String::new(), error: None });
        handle_key(&mut state, key(Key::Char('x')), &tx);
        if let Screen::BrowsingLobbies(BrowsingLobbiesState { status: BrowsingStatus::EnteringId { input, .. }, .. }) = &state.screen {
            assert_eq!(input, "x");
        } else {
            panic!("unexpected state");
        }
    }

    #[test]
    fn entering_id_backspace_pops_char() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::EnteringId { input: "ab".to_string(), error: None });
        handle_key(&mut state, key(Key::Backspace), &tx);
        if let Screen::BrowsingLobbies(BrowsingLobbiesState { status: BrowsingStatus::EnteringId { input, .. }, .. }) = &state.screen {
            assert_eq!(input, "a");
        } else {
            panic!("unexpected state");
        }
    }

    #[test]
    fn entering_id_esc_resets_to_loading() {
        let tx = make_tx();
        let mut state = browsing_state("Alice", BrowsingStatus::EnteringId { input: "some-id".to_string(), error: None });
        handle_key(&mut state, key(Key::Esc), &tx);
        if let Screen::BrowsingLobbies(b) = &state.screen {
            assert!(matches!(b.status, BrowsingStatus::Loading));
        } else {
            panic!("expected BrowsingLobbies");
        }
    }
}
