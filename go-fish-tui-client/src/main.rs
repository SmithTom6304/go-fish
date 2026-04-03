mod event_loop;
mod input;
mod network;
mod state;
mod ui;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Config {
    pub server_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: "ws://terminaltom.com/go-fish/game-server".to_string(),
        }
    }
}

// ── Native entry point ────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use crate::Config;
    use std::io::Stdout;
    use std::path::PathBuf;

    use clap::Parser;
    use ratatui::{backend::CrosstermBackend, Terminal};

    use crate::event_loop::run_event_loop;

    #[derive(Parser)]
    #[command(name = "go-fish-tui-client")]
    struct Cli {
        /// Path to a TOML config file
        #[arg(long)]
        config: Option<PathBuf>,
    }

    fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
        Ok(Terminal::new(CrosstermBackend::new(stdout))?)
    }

    fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(terminal.backend_mut(), crossterm::terminal::LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }

    pub async fn run() -> anyhow::Result<()> {
        let cli = Cli::parse();

        let config = match cli.config {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<Config>(&contents) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        eprintln!("Warning: failed to parse {}: {}, using defaults", path.display(), e);
                        Config::default()
                    }
                },
                Err(e) => {
                    eprintln!("Warning: failed to read {}: {}, using defaults", path.display(), e);
                    Config::default()
                }
            },
            None => Config::default(),
        };

        if !config.server_url.starts_with("ws://") && !config.server_url.starts_with("wss://") {
            eprintln!("Error: server URL must start with ws:// or wss://");
            std::process::exit(1);
        }

        let (ws, _response) = tokio_tungstenite::connect_async(&config.server_url)
            .await
            .unwrap_or_else(|e| {
                eprintln!("Error: failed to connect to {}: {}", config.server_url, e);
                std::process::exit(1);
            });

        let (client_msg_tx, client_msg_rx) =
            tokio::sync::mpsc::channel::<go_fish_web::ClientMessage>(32);
        let (network_event_tx, network_event_rx) =
            tokio::sync::mpsc::channel::<crate::network::NetworkEvent>(32);

        tokio::spawn(crate::network::run_network_task(ws, network_event_tx, client_msg_rx));

        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::LeaveAlternateScreen
            );
            original_hook(panic_info);
        }));

        let mut terminal = setup_terminal()?;
        run_event_loop(&mut terminal, network_event_rx, client_msg_tx).await?;
        restore_terminal(&mut terminal)?;

        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    native::run().await
}

// ── WASM entry point ──────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gloo_net::websocket::futures::WebSocket;
    use ratatui::Terminal;
    use ratzilla::{DomBackend, WebRenderer};
    use tokio::sync::mpsc;
    use wasm_bindgen::prelude::wasm_bindgen;
    use wasm_bindgen_futures::spawn_local;

    use go_fish_web::ClientMessage;

    use crate::input::{handle_key, KeyInput};
    use crate::network::NetworkEvent;
    use crate::state::{apply_network_event, AppState, Screen};
    use crate::ui::render;

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen]
    extern "C" {}

    pub fn run(server_url: &str) {
        let ws = WebSocket::open(server_url).expect("failed to connect to server");

        let (client_msg_tx, client_msg_rx) = mpsc::channel::<ClientMessage>(32);
        let (network_event_tx, network_event_rx) = mpsc::channel::<NetworkEvent>(32);

        spawn_local(crate::network::run_network_task(ws, network_event_tx, client_msg_rx));

        // Shared state between the key-event callback and the render callback.
        let state = Rc::new(RefCell::new(AppState::new()));

        // Send the initial Identity message and update connecting status.
        {
            let mut s = state.borrow_mut();
            if let Screen::Connecting(ref mut cs) = s.screen {
                cs.status = "Negotiating identity…".to_string();
            }
        }
        let _ = client_msg_tx.try_send(ClientMessage::Identity);

        // Drain network events into the shared state from within the render loop.
        // `network_event_rx` is moved into the render closure via Rc<RefCell<...>>.
        let network_rx = Rc::new(RefCell::new(network_event_rx));

        let backend = DomBackend::new().expect("failed to create DOM backend");
        let mut terminal = Terminal::new(backend).expect("failed to create terminal");

        // Key event callback.
        let state_k = state.clone();
        let tx_k = client_msg_tx.clone();
        terminal.on_key_event(move |key_event| {
            handle_key(&mut state_k.borrow_mut(), KeyInput::from(key_event), &tx_k);
        });

        // Render loop — also drains pending network events each frame.
        // draw_web consumes the terminal and drives requestAnimationFrame.
        terminal.draw_web(move |f| {
            // Drain all pending network events before rendering.
            let mut rx = network_rx.borrow_mut();
            while let Ok(event) = rx.try_recv() {
                apply_network_event(&mut state.borrow_mut(), &event);
            }

            render(f, &state.borrow());
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // cannot use config, fs is stubbed out for wasm
    // configurable server url would require fetching via http
    wasm::run("ws://terminaltom.com/go-fish/game-server");
}
