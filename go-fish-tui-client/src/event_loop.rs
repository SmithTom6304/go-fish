#[cfg(not(target_arch = "wasm32"))]
pub use native::run_event_loop;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::time::Duration;

    use crossterm::event::{poll, read, Event};
    use ratatui::{backend::Backend, Terminal};
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::error::TryRecvError;

    use go_fish_web::ClientMessage;

    use crate::input::handle_key;
    use crate::network::NetworkEvent;
    use crate::state::{apply_network_event, AppState, Screen};
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

        if let Screen::Connecting(ref mut s) = state.screen {
            s.status = "Negotiating identity…".to_string();
        }

        loop {
            terminal.draw(|f| render(f, &state))?;

            if poll(Duration::from_millis(50))? {
                if let Event::Key(key) = read()? {
                    if handle_key(&mut state, key.into(), &client_msg_tx) {
                        break;
                    }
                }
            }

            match network_event_rx.try_recv() {
                Ok(event) => apply_network_event(&mut state, &event),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
        }

        Ok(())
    }
}
