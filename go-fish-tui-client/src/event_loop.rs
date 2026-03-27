use std::time::Duration;

use crossterm::event::{poll, read, Event, KeyCode, KeyModifiers};
use ratatui::{backend::Backend, Terminal};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

use go_fish_web::ClientMessage;

use crate::network::NetworkEvent;
use crate::state::{apply_network_event, is_valid_lobby_id, AppState, PreLobbyInputLobbyIdState, Screen};
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
                        Screen::Lobby(_) => {
                            if key.code == KeyCode::Char('l') {
                                let _ = client_msg_tx.send(ClientMessage::LeaveLobby).await;
                            }
                        }
                        Screen::Connecting(_) => {}
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
