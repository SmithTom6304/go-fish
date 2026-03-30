use std::time::Duration;

use crossterm::event::{poll, read, Event, KeyCode, KeyModifiers};
use ratatui::{backend::Backend, Terminal};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use go_fish_web::ClientMessage;

use crate::network::NetworkEvent;
use crate::state::{apply_network_event, is_valid_lobby_id, AppState, GameInputState, PreLobbyInputLobbyIdState, Screen};
use crate::ui::render;

pub async fn run_event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    mut network_event_rx: mpsc::Receiver<NetworkEvent>,
    client_msg_tx: mpsc::Sender<ClientMessage>,
) -> anyhow::Result<()>
where
    B::Error: Send + Sync + 'static,
{
    // Send Identity message to begin the handshake
    let _ = client_msg_tx.send(ClientMessage::Identity).await;

    let mut state = AppState::new();

    // Update status to indicate identity negotiation is in progress
    if let Screen::Connecting(ref mut s) = state.screen {
        s.status = "Negotiating identity…".to_string();
    }

    loop {
        // Render current state
        terminal.draw(|f| render(f, &state))?;

        // Poll for terminal input events
        if poll(Duration::from_millis(50))? {
            match read()? {
                Event::Key(key) => {
                    // Quit: Ctrl+C on any screen
                    let is_quit = key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL);

                    if is_quit {
                        if let Screen::Lobby(_) = &state.screen {
                            let _ = client_msg_tx.send(ClientMessage::LeaveLobby).await;
                        }
                        // Game screen: do NOT send LeaveLobby (server handles disconnection)
                        break;
                    }

                    match &mut state.screen {
                        Screen::PreLobby(pre) => {
                            match &mut pre.input_state {
                                crate::state::PreLobbyInputState::None => {
                                    if key.code == KeyCode::Char('c') {
                                        let _ = client_msg_tx.send(ClientMessage::CreateLobby).await;
                                    } else if key.code == KeyCode::Char('q') {
                                        let _ = client_msg_tx.send(ClientMessage::LeaveLobby).await;
                                        break;
                                    } else if key.code == KeyCode::Char('j') {
                                        pre.input_state = crate::state::PreLobbyInputState::LobbyId(PreLobbyInputLobbyIdState::default());
                                    }
                                }
                                crate::state::PreLobbyInputState::LobbyId(lobby_id_state) => {
                                    let lobby_id = &mut lobby_id_state.lobby_id;
                                    match key.code {
                                        KeyCode::Char(ch) => {
                                            lobby_id.push(ch);
                                            lobby_id_state.error = None;
                                        },
                                        KeyCode::Backspace => {
                                            lobby_id.pop();
                                        },
                                        KeyCode::Enter => {
                                            if is_valid_lobby_id(lobby_id) {
                                                let lobby_id = lobby_id.trim().to_string();
                                                let _ = client_msg_tx
                                                    .send(ClientMessage::JoinLobby(lobby_id))
                                                    .await;
                                            } else {
                                                lobby_id_state.error =
                                                    Some("Please enter a valid Lobby ID".to_string());
                                            }
                                        },
                                        KeyCode::Esc => {
                                            pre.input_state = crate::state::PreLobbyInputState::None;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        Screen::Lobby(lobby) => {
                            // Task 7.1: s key sends StartGame if leader with ≥2 players
                            if key.code == KeyCode::Char('s')
                                && lobby.players.len() >= 2
                                && lobby.leader == lobby.player_name
                            {
                                let _ = client_msg_tx.send(ClientMessage::StartGame).await;
                            } else if key.code == KeyCode::Char('q') {
                                let _ = client_msg_tx.send(ClientMessage::LeaveLobby).await;
                            }
                        }
                        Screen::Connecting(_) => {}
                        Screen::Game(game) => {
                            // Task 7.3: game result acknowledgement (checked first)
                            if game.game_result.is_some() {
                                if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                                    let player_name = game.player_name.clone();
                                    state.screen = crate::state::Screen::PreLobby(crate::state::PreLobbyState {
                                        player_name,
                                        input_state: crate::state::PreLobbyInputState::None,
                                        error: None,
                                    });
                                } else if key.code == KeyCode::Char('q') {
                                    break;
                                }
                            } else {
                                // Task 7.2: hook input navigation
                                match &game.input_state {
                                    GameInputState::Idle => {
                                        if key.code == KeyCode::Char('q') {
                                            break;
                                        }
                                        let opponents: Vec<String> = game.players.iter()
                                            .filter(|p| *p != &game.player_name)
                                            .cloned()
                                            .collect();
                                        // Start hook input navigation
                                        if key.code == KeyCode::Char('h') && opponents.len() > 0 && game.active_player == game.player_name {
                                            game.input_state = if opponents.len() == 1 {
                                                // If only 1 opponent, can skip select target stage
                                                GameInputState::SelectingRank { target: opponents[0].clone(), cursor: 0 }
                                            } else {
                                                GameInputState::SelectingTarget { cursor: 0 }
                                            }
                                        }
                                    }
                                    GameInputState::SelectingTarget { cursor } => {
                                        let opponents: Vec<String> = game.players.iter()
                                            .filter(|p| *p != &game.player_name)
                                            .cloned()
                                            .collect();
                                        let n = opponents.len();
                                        if n == 0 { return Ok(()); }
                                        let cursor = *cursor;
                                        match key.code {
                                            KeyCode::Char('j') | KeyCode::Down => {
                                                game.input_state = GameInputState::SelectingTarget {
                                                    cursor: (cursor + 1) % n,
                                                };
                                            }
                                            KeyCode::Char('k') | KeyCode::Up => {
                                                game.input_state = GameInputState::SelectingTarget {
                                                    cursor: (cursor + n - 1) % n,
                                                };
                                            }
                                            KeyCode::Enter => {
                                                if game.hand.books.is_empty() {
                                                    // No cards to ask for — stay in SelectingTarget
                                                } else {
                                                    let target = opponents[cursor].clone();
                                                    game.input_state = GameInputState::SelectingRank {
                                                        target,
                                                        cursor: 0,
                                                    };
                                                }
                                            }
                                            KeyCode::Esc => {
                                                game.input_state = GameInputState::Idle;
                                            }
                                            KeyCode::Char('q') => {
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                    GameInputState::SelectingRank { target, cursor } => {
                                        let cards = game.hand.books.iter().flat_map(|b| b.cards.clone()).collect::<Vec<_>>();
                                        let m = game.hand.books.iter().map(|b| b.cards.len()).sum::<usize>();
                                        if m == 0 {
                                            game.input_state = GameInputState::SelectingTarget { cursor: 0 };
                                        } else {
                                            let cursor = *cursor;
                                            let target = target.clone();
                                            match key.code {
                                                KeyCode::Char('l') | KeyCode::Right => {
                                                    game.input_state = GameInputState::SelectingRank {
                                                        target,
                                                        cursor: (cursor + 1) % m,
                                                    };
                                                }
                                                KeyCode::Char('h') | KeyCode::Left => {
                                                    game.input_state = GameInputState::SelectingRank {
                                                        target,
                                                        cursor: (cursor + m - 1) % m,
                                                    };
                                                }
                                                KeyCode::Enter => {
                                                    let rank = cards[cursor].rank;
                                                    let _ = client_msg_tx.send(ClientMessage::Hook(
                                                        go_fish_web::ClientHookRequest { target_name: target, rank }
                                                    )).await;
                                                    game.input_state = GameInputState::Idle;
                                                }
                                                KeyCode::Esc => {
                                                    game.input_state = GameInputState::SelectingTarget { cursor: 0 };
                                                }
                                                KeyCode::Char('q') => {
                                                    break;
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Event::Resize(_, _) => {
                    // No-op: next draw handles the new dimensions
                }
                _ => {}
            }
        }

        // Poll network events non-blocking
        match network_event_rx.try_recv() {
            Ok(event) => {
                apply_network_event(&mut state, &event);
            }
            Err(TryRecvError::Empty) => {
                // Nothing to process, continue
            }
            Err(TryRecvError::Disconnected) => {
                break;
            }
        }
    }

    Ok(())
}
