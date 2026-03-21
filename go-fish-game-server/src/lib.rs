use go_fish_web::ClientMessage;
use std::net::SocketAddr;

#[derive(Debug)]
pub struct AddressedClientMessage {
    pub client: SocketAddr,
    pub client_message: ClientMessage,
}