use go_fish_web::LobbyLeftReason;
use go_fish_web::ServerMessage;

pub use crate::network::NetworkEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub server_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: "ws://127.0.0.1:9001".to_string(),
        }
    }
}

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
    LobbyId(String)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LobbyState {
    pub player_name: String,
    pub lobby_id: String,
    pub leader: String,
    pub players: Vec<String>,
    pub max_players: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Connecting(ConnectingState),
    PreLobby(PreLobbyState),
    Lobby(LobbyState),
}

#[derive(Debug, Clone, PartialEq)]
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
                if !players.contains(&player_name) {
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
        ServerMessage::Error(msg) => {
            match &mut state.screen {
                Screen::Connecting(s) => {
                    s.status = msg.clone();
                }
                Screen::PreLobby(s) => {
                    s.error = Some(msg.clone());
                }
                Screen::Lobby(s) => {
                    s.error = Some(msg.clone());
                }
            }
        }
        // Other server messages are not handled at the state level yet
        _ => {}
    }
}

fn apply_connection_closed(state: &mut AppState) {
    let msg = "Server closed connection.".to_string();
    match &mut state.screen {
        Screen::Connecting(s) => s.status = msg,
        Screen::PreLobby(s) => s.error = Some(msg),
        Screen::Lobby(s) => s.error = Some(msg),
    }
}

fn apply_connection_error(state: &mut AppState, err: &str) {
    let msg = format!("Connection error: {}", err);
    match &mut state.screen {
        Screen::Connecting(s) => s.status = msg,
        Screen::PreLobby(s) => s.error = Some(msg),
        Screen::Lobby(s) => s.error = Some(msg),
    }
}

/// Returns true if the lobby ID is non-empty after trimming whitespace.
pub fn is_valid_lobby_id(s: &str) -> bool {
    !s.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn new_starts_in_connecting_state() {
        let state = AppState::new();
        assert!(matches!(state.screen, Screen::Connecting(_)));
        if let Screen::Connecting(s) = &state.screen {
            assert_eq!(s.status, "Connecting…");
        }
    }

    #[test]
    fn player_identity_transitions_to_pre_lobby() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Alice".to_string())),
        );
        assert!(matches!(state.screen, Screen::PreLobby(_)));
        if let Screen::PreLobby(s) = &state.screen {
            assert_eq!(s.player_name, "Alice");
            assert_eq!(s.input_state, PreLobbyInputState::None);
            assert_eq!(s.error, None);
        }
    }

    #[test]
    fn lobby_joined_transitions_to_lobby() {
        let mut state = AppState::new();
        // First get to PreLobby
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Bob".to_string())),
        );
        // Then join a lobby
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyJoined {
                lobby_id: "LOBBY1".to_string(),
                leader: "Bob".to_string(),
                players: vec!["Bob".to_string(), "Carol".to_string()],
                max_players: 4,
            }),
        );
        assert!(matches!(state.screen, Screen::Lobby(_)));
        if let Screen::Lobby(s) = &state.screen {
            assert_eq!(s.player_name, "Bob");
            assert_eq!(s.lobby_id, "LOBBY1");
            assert_eq!(s.leader, "Bob");
            assert_eq!(s.players, vec!["Bob", "Carol"]);
            assert_eq!(s.max_players, 4);
            assert_eq!(s.error, None);
        }
    }

    #[test]
    fn lobby_updated_updates_leader_and_players_only() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Bob".to_string())),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyJoined {
                lobby_id: "LOBBY1".to_string(),
                leader: "Bob".to_string(),
                players: vec!["Bob".to_string(), "Carol".to_string()],
                max_players: 4,
            }),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyUpdated {
                leader: "Carol".to_string(),
                players: vec!["Bob".to_string(), "Carol".to_string(), "Dave".to_string()],
            }),
        );
        if let Screen::Lobby(s) = &state.screen {
            assert_eq!(s.leader, "Carol");
            assert_eq!(s.players, vec!["Bob", "Carol", "Dave"]);
            // Unchanged fields
            assert_eq!(s.player_name, "Bob");
            assert_eq!(s.lobby_id, "LOBBY1");
            assert_eq!(s.max_players, 4);
        } else {
            panic!("Expected Lobby screen");
        }
    }

    #[test]
    fn error_on_pre_lobby_sets_error_does_not_navigate() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Alice".to_string())),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::Error("Lobby not found".to_string())),
        );
        assert!(matches!(state.screen, Screen::PreLobby(_)));
        if let Screen::PreLobby(s) = &state.screen {
            assert_eq!(s.error, Some("Lobby not found".to_string()));
        }
    }

    #[test]
    fn error_on_lobby_sets_error_does_not_navigate() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Alice".to_string())),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyJoined {
                lobby_id: "L1".to_string(),
                leader: "Alice".to_string(),
                players: vec!["Alice".to_string()],
                max_players: 2,
            }),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::Error("Game already started".to_string())),
        );
        assert!(matches!(state.screen, Screen::Lobby(_)));
        if let Screen::Lobby(s) = &state.screen {
            assert_eq!(s.error, Some("Game already started".to_string()));
        }
    }

    #[test]
    fn is_valid_lobby_id_rejects_empty_and_whitespace() {
        assert!(!is_valid_lobby_id(""));
        assert!(!is_valid_lobby_id("   "));
        assert!(!is_valid_lobby_id("\t\n"));
        assert!(is_valid_lobby_id("ABC12"));
        assert!(is_valid_lobby_id("  ABC12  "));
    }

    #[test]
    fn lobby_updated_removes_local_player_transitions_to_pre_lobby() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Alice".to_string())),
        );
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyJoined {
                lobby_id: "L1".to_string(),
                leader: "Alice".to_string(),
                players: vec!["Alice".to_string(), "Bob".to_string()],
                max_players: 4,
            }),
        );
        // LobbyUpdated that removes Alice (local player)
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::LobbyUpdated {
                leader: "Bob".to_string(),
                players: vec!["Bob".to_string()],
            }),
        );
        assert!(matches!(state.screen, Screen::PreLobby(_)));
        if let Screen::PreLobby(s) = &state.screen {
            assert_eq!(s.player_name, "Alice");
        }
    }

    #[test]
    fn network_closed_sets_status_on_connecting() {
        let mut state = AppState::new();
        apply_network_event(&mut state, &NetworkEvent::Closed);
        if let Screen::Connecting(s) = &state.screen {
            assert!(s.status.contains("closed") || s.status.contains("Server"));
        } else {
            panic!("Expected Connecting screen");
        }
    }

    #[test]
    fn network_error_sets_error_on_pre_lobby() {
        let mut state = AppState::new();
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::PlayerIdentity("Alice".to_string())),
        );
        apply_network_event(&mut state, &NetworkEvent::Error("timeout".to_string()));
        if let Screen::PreLobby(s) = &state.screen {
            assert!(s.error.is_some());
        } else {
            panic!("Expected PreLobby screen");
        }
    }

    // Feature: go-fish-tui-client, Property 5: PlayerIdentity produces correct PreLobbyState
    proptest! {
        #[test]
        fn prop_player_identity_produces_correct_pre_lobby_state(name in any::<String>()) {
            let mut state = AppState::new();
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::PlayerIdentity(name.clone())),
            );
            if let Screen::PreLobby(s) = &state.screen {
                prop_assert_eq!(&s.player_name, &name);
                prop_assert_eq!(&s.input_state, &PreLobbyInputState::None);
                prop_assert_eq!(s.error.clone(), None);
            } else {
                return Err(TestCaseError::fail("Expected PreLobby screen"));
            }
        }
    }

    // Feature: go-fish-tui-client, Property 6: LobbyJoined produces correct LobbyState
    proptest! {
        #[test]
        fn prop_lobby_joined_produces_correct_lobby_state(
            lobby_id in any::<String>(),
            leader in any::<String>(),
            players in proptest::collection::vec(any::<String>(), 0..10),
            max_players in any::<usize>(),
        ) {
            let mut state = AppState::new();
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::PlayerIdentity("TestPlayer".to_string())),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::LobbyJoined {
                    lobby_id: lobby_id.clone(),
                    leader: leader.clone(),
                    players: players.clone(),
                    max_players,
                }),
            );
            if let Screen::Lobby(s) = &state.screen {
                prop_assert_eq!(&s.lobby_id, &lobby_id);
                prop_assert_eq!(&s.leader, &leader);
                prop_assert_eq!(&s.players, &players);
                prop_assert_eq!(s.max_players, max_players);
            } else {
                return Err(TestCaseError::fail("Expected Lobby screen"));
            }
        }
    }

    // Feature: go-fish-tui-client, Property 7: LobbyUpdated mutates only leader and players
    proptest! {
        #[test]
        fn prop_lobby_updated_mutates_only_leader_and_players(
            new_leader in any::<String>(),
            mut new_players in proptest::collection::vec(any::<String>(), 0..10),
        ) {
            // Ensure local player is in new_players so no transition back to PreLobby
            new_players.push("TestPlayer".to_string());

            let fixed_lobby_id = "FIXED_LOBBY".to_string();
            let fixed_max_players: usize = 4;

            let mut state = AppState::new();
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::PlayerIdentity("TestPlayer".to_string())),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::LobbyJoined {
                    lobby_id: fixed_lobby_id.clone(),
                    leader: "OriginalLeader".to_string(),
                    players: vec!["TestPlayer".to_string()],
                    max_players: fixed_max_players,
                }),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::LobbyUpdated {
                    leader: new_leader.clone(),
                    players: new_players.clone(),
                }),
            );
            if let Screen::Lobby(s) = &state.screen {
                prop_assert_eq!(&s.leader, &new_leader);
                prop_assert_eq!(&s.players, &new_players);
                // Unchanged fields
                prop_assert_eq!(&s.lobby_id, &fixed_lobby_id);
                prop_assert_eq!(&s.player_name, "TestPlayer");
                prop_assert_eq!(s.max_players, fixed_max_players);
            } else {
                return Err(TestCaseError::fail("Expected Lobby screen after LobbyUpdated"));
            }
        }
    }

    // Feature: go-fish-tui-client, Property 8: Whitespace-only Lobby_Id is not submitted
    proptest! {
        #[test]
        fn prop_whitespace_only_lobby_id_is_invalid(
            s in proptest::string::string_regex("[ \\t\\n\\r]*").unwrap(),
        ) {
            prop_assert!(!is_valid_lobby_id(&s));
        }
    }

    // Feature: go-fish-tui-client, Property 9: Error on Pre-Lobby screen does not navigate
    proptest! {
        #[test]
        fn prop_error_on_pre_lobby_does_not_navigate(err in any::<String>()) {
            let mut state = AppState::new();
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::PlayerIdentity("TestPlayer".to_string())),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::Error(err.clone())),
            );
            if let Screen::PreLobby(s) = &state.screen {
                prop_assert_eq!(s.error.clone(), Some(err));
            } else {
                return Err(TestCaseError::fail("Expected PreLobby screen after Error"));
            }
        }
    }

    // Feature: go-fish-tui-client, Property 10: Error on Lobby screen does not navigate
    proptest! {
        #[test]
        fn prop_error_on_lobby_does_not_navigate(err in any::<String>()) {
            let mut state = AppState::new();
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::PlayerIdentity("TestPlayer".to_string())),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::LobbyJoined {
                    lobby_id: "L1".to_string(),
                    leader: "TestPlayer".to_string(),
                    players: vec!["TestPlayer".to_string()],
                    max_players: 2,
                }),
            );
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::Error(err.clone())),
            );
            if let Screen::Lobby(s) = &state.screen {
                prop_assert_eq!(s.error.clone(), Some(err));
            } else {
                return Err(TestCaseError::fail("Expected Lobby screen after Error"));
            }
        }
    }
}
