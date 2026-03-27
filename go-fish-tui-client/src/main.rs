mod event_loop;
mod network;
mod state;
mod ui;

use std::io::Stdout;

use clap::Parser;
use ratatui::{Terminal, backend::CrosstermBackend};

#[derive(Parser)]
struct Config {
    #[arg(long, default_value = "ws://127.0.0.1:9001")]
    server_url: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    // Validate WebSocket URL
    if !config.server_url.starts_with("ws://") && !config.server_url.starts_with("wss://") {
        eprintln!("Error: server URL must start with ws:// or wss://");
        std::process::exit(1);
    }

    // Connect to the server
    let (ws, _response) = tokio_tungstenite::connect_async(&config.server_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to connect to {}: {}", config.server_url, e);
            std::process::exit(1);
        });

    // Create channels
    let (client_msg_tx, client_msg_rx) =
        tokio::sync::mpsc::channel::<go_fish_web::ClientMessage>(32);
    let (network_event_tx, network_event_rx) =
        tokio::sync::mpsc::channel::<crate::network::NetworkEvent>(32);

    // Spawn the network task
    tokio::spawn(crate::network::run_network_task(
        ws,
        network_event_tx,
        client_msg_rx,
    ));

    // Install panic hook to restore terminal on panic
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
    crate::event_loop::run_event_loop(&mut terminal, network_event_rx, client_msg_tx).await?;
    restore_terminal(&mut terminal)?;

    Ok(())
}
