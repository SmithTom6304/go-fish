pub mod connection;
pub mod lobby;

use serde::Deserialize;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::info;

pub use connection::{
    ClientEvent, ClientHandle, ConnectionManager, DisconnectReason,
    LobbyEvent, ManagerCommand, ServerMessage,
};
pub use lobby::{
    ClientPhase, Lobby, LobbyCommand, LobbyManager, PlayerRecord,
};

#[derive(Debug, Deserialize)]
pub struct SimpleBotConfig {
    pub memory_limit: u8,
    pub error_margin: f32,
}

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub thinking_time_min_ms: u64,
    pub thinking_time_max_ms: u64,
    pub simple_bot: SimpleBotConfig,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub address: SocketAddr,
    pub lobby_max_players: usize,
    pub max_client_connections: usize,
    pub bots: Option<BotConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "127.0.0.1:9001".parse().unwrap(),
            lobby_max_players: 4,
            max_client_connections: 10,
            bots: None,
        }
    }
}

pub async fn run(config: Config) -> Result<(), anyhow::Error> {
    let (lobby_event_tx, lobby_event_rx) = mpsc::channel::<LobbyEvent>(64);
    let (lobby_cmd_tx, lobby_cmd_rx) = mpsc::channel::<LobbyCommand>(8);

    let manager = ConnectionManager::new(lobby_event_tx.clone(), config.max_client_connections);
    let event_tx = manager.event_tx();
    let command_tx = manager.command_tx();

    let bot_config = config.bots.unwrap_or(BotConfig {
        thinking_time_min_ms: 2000,
        thinking_time_max_ms: 4500,
        simple_bot: SimpleBotConfig { memory_limit: 3, error_margin: 0.2 },
    });

    let lobby_manager = LobbyManager::new(
        lobby_event_rx,
        lobby_cmd_rx,
        config.lobby_max_players,
        lobby_event_tx.clone(),
        bot_config,
    );

    let (listener_cmd_tx, listener_cmd_rx) = mpsc::channel::<ManagerCommand>(1);
    tokio::spawn(connection::run_tcp_listener(config.address, event_tx, listener_cmd_rx));
    tokio::spawn(lobby_manager.run());

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    async fn shutdown(
        listener_cmd_tx: mpsc::Sender<ManagerCommand>,
        command_tx: mpsc::Sender<ManagerCommand>,
        lobby_cmd_tx: mpsc::Sender<LobbyCommand>,
    ) {
        let _ = listener_cmd_tx.send(ManagerCommand::Shutdown).await;
        let _ = command_tx.send(ManagerCommand::Shutdown).await;
        let _ = lobby_cmd_tx.send(LobbyCommand::Shutdown).await;
    }

    tokio::select! {
        _ = manager.run() => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received SIGINT, shutting down gracefully");
            shutdown(listener_cmd_tx, command_tx, lobby_cmd_tx).await;
        }
        _ = sigterm.recv() => {
            info!("received SIGTERM, shutting down gracefully");
            shutdown(listener_cmd_tx, command_tx, lobby_cmd_tx).await;
        }
    }

    Ok(())
}
