pub mod connection;
pub mod lobby;

use serde::Deserialize;
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tracing::info;

pub use connection::{
    ClientEvent, ClientHandle, ConnectionManager, DisconnectReason,
    LobbyEvent, LobbyOutboundMessage, ManagerCommand, ServerMessage,
};
pub use lobby::{
    ClientPhase, Lobby, LobbyCommand, LobbyManager, LobbyState, PlayerRecord,
};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub address: SocketAddr,
    pub lobby_max_players: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "127.0.0.1:9001".parse().unwrap(),
            lobby_max_players: 4,
        }
    }
}

pub async fn run(config: Config) -> Result<(), anyhow::Error> {
    let (lobby_event_tx, lobby_event_rx) = mpsc::channel::<LobbyEvent>(64);
    let (lobby_outbound_tx, lobby_outbound_rx) = mpsc::channel::<LobbyOutboundMessage>(64);
    let (lobby_cmd_tx, lobby_cmd_rx) = mpsc::channel::<LobbyCommand>(8);

    let manager = ConnectionManager::new(lobby_event_tx, lobby_outbound_rx);
    let event_tx = manager.event_tx();
    let command_tx = manager.command_tx();

    let lobby_manager = LobbyManager::new(
        lobby_event_rx,
        lobby_outbound_tx,
        lobby_cmd_rx,
        config.lobby_max_players,
    );

    let (listener_cmd_tx, listener_cmd_rx) = mpsc::channel::<ManagerCommand>(1);
    tokio::spawn(connection::run_tcp_listener(config.address, event_tx, listener_cmd_rx));
    tokio::spawn(lobby_manager.run());

    tokio::select! {
        _ = manager.run() => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received Ctrl+C, shutting down gracefully");
            let _ = listener_cmd_tx.send(ManagerCommand::Shutdown).await;
            let _ = command_tx.send(ManagerCommand::Shutdown).await;
            let _ = lobby_cmd_tx.send(LobbyCommand::Shutdown).await;
        }
    }

    Ok(())
}
