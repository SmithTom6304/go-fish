use go_fish_web::ClientMessage;
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug)]
pub struct AddressedClientMessage {
    pub client: SocketAddr,
    pub client_message: ClientMessage,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub address: SocketAddr,
    pub player_count: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: SocketAddr::from(([127, 0, 0, 1], 9001)),
            player_count: 2,
        }
    }
}