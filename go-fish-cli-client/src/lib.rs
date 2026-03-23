use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server_address: SocketAddr,
    pub name: Option<String>
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_address: SocketAddr::from(([127, 0, 0, 1], 9001)),
            name: None
        }
    }
}