use futures_util::{SinkExt, StreamExt};
use go_fish::{Deck, Game, Hook, PlayerId};
use go_fish_web::{ClientMessage, GameResult};
use go_fish_web::{FullHookRequest, HookAndResult, ServerMessage};
use go_fish_web::{PlayerState, PlayerTurnValue};
use log::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, tungstenite::{Error, Result}, WebSocketStream};

async fn accept_connection(peer: SocketAddr, stream: TcpStream, websocket_communicator: WebsocketCommunicator) {
    if let Err(e) = handle_connection(peer, stream, websocket_communicator).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8(_) => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(peer: SocketAddr, stream: TcpStream, websocket_communicator: WebsocketCommunicator) -> Result<()> {
    let ws_stream = accept_async(stream).await.expect("Failed to accept");

    info!("New WebSocket connection: {}", peer);

    run_websocket(peer, websocket_communicator, ws_stream).await;

    Ok(())
}

struct ControllerCommunicator {
    pub hook_rx: mpsc::Receiver<Message>,
    pub clients_tx: HashMap<PlayerId, mpsc::Sender<Message>>,
    pub client_broadcast_tx: broadcast::Sender<Message>,
}

struct ControllerLookup {
    pub port_to_name: HashMap<u16, String>,
    pub name_to_player_id: HashMap<String, PlayerId>,
    pub player_id_to_name: HashMap<PlayerId, String>,
}

struct WebsocketCommunicator {
    pub hook_tx: mpsc::Sender<Message>,
    pub client_rx: mpsc::Receiver<Message>,
    pub client_broadcast_rx: broadcast::Receiver<Message>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InternalClientMessage {
    client: SocketAddr,
    message: ClientMessage
}

async fn send_server_message(message: ServerMessage, tx: &mpsc::Sender<Message>) {
    let json = serde_json::to_string(&message).unwrap();
    _ = tx.send(Message::Text(json.into())).await;
}

fn broadcast_server_message(message: ServerMessage, tx: &broadcast::Sender<Message>) {
    let json = serde_json::to_string(&message).unwrap();
    _ = tx.send(Message::Text(json.into()));
}

async fn send_pregame_messages(game: &Game, comm: &mut ControllerCommunicator, lookup: & ControllerLookup) {
    let current_player = game.get_current_player();
    let current_player_name = lookup.player_id_to_name[&current_player.id].clone();
    for player in game.players.iter().clone() {
        let tx = &comm.clients_tx[&player.id];
        let state: PlayerState = PlayerState {
            hand: player.hand.clone(),
            completed_books: player.completed_books.clone(),
        };

        let player_name = lookup.player_id_to_name[&player.id].clone();
        log::info!("Sending player identity to: {:?}", player.id);
        let msg = ServerMessage::PlayerIdentity(player_name);
        send_server_message(msg, tx).await;

        log::info!("Sending client state to: {:?}", player.id);
        let msg = ServerMessage::PlayerState(state);
        send_server_message(msg, tx).await;

        log::info!("Sending player turn state to: {:?}", player.id);
        let msg = match player.id == current_player.id {
            true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
            false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.clone())),
        };
        send_server_message(msg, tx).await;
    }
}

async fn run_controller(mut comm: ControllerCommunicator, lookup: ControllerLookup) {
    let mut deck = Deck::new();
    deck.shuffle();
    let mut game = Game::new(deck, 2);

    send_pregame_messages(&game, &mut comm, &lookup).await;

    while let Some(msg) = comm.hook_rx.recv().await {
        log::info!("Received hook message: {:?}", msg);
        let icm = msg.to_text().unwrap();
        let icm = serde_json::from_str::<InternalClientMessage>(icm).unwrap();

        let client_name = lookup.port_to_name[&icm.client.port()].clone();
        let client_player_id = lookup.name_to_player_id[&client_name];

        match icm.message {
            ClientMessage::Hook(hook) => {
                let current_player = game.get_current_player();
                if current_player.id != client_player_id {
                    // TODO Not your turn!
                    log::info!("Not player {:?} turn!", client_player_id);
                    continue;
                };
                let target_name = hook.target_name;
                let target_player_id = lookup.name_to_player_id[&target_name];
                if current_player.id == target_player_id {
                    // TODO Cannot target yourself!
                    log::info!("Player {:?} cannot target themselves!", client_player_id);
                    continue;
                }

                if !current_player.hand.books.iter().any(|book| book.rank == hook.rank) {
                    // TODO Cannot ask for a card you do not have!
                    log::info!("Player {:?} has no cards of rank {:?}!", client_player_id, hook.rank);
                    continue;
                }

                let rank = hook.rank;
                let hook = Hook { target: target_player_id, rank: hook.rank };
                let result = game.take_turn(hook).unwrap();

                let full_request = FullHookRequest {
                    fisher_name: client_name,
                    target_name,
                    rank
                };

                let hook_result_message = ServerMessage::HookAndResult(HookAndResult {hook_request: full_request, hook_result: result});
                broadcast_server_message(hook_result_message, &comm.client_broadcast_tx);

                let current_player = game.get_current_player();
                let current_player_name = lookup.player_id_to_name[&current_player.id].clone();

                for player in game.players.iter().clone() {
                    let tx = &comm.clients_tx[&player.id];
                    let state: PlayerState = PlayerState {
                        hand: player.hand.clone(),
                        completed_books: player.completed_books.clone(),
                    };
                    log::info!("Sending client state to: {:?}", player.id);
                    let msg = ServerMessage::PlayerState(state);
                    send_server_message(msg, tx).await;

                    log::info!("Sending player turn state to: {:?}", player.id);
                    let msg = match player.id == current_player.id {
                        true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
                        false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.clone())),
                    };
                    send_server_message(msg, tx).await;
                }
            }
        }

        if game.is_finished {
            log::info!("Game finished!");
            let result = game.get_game_result().unwrap();
            let winners = result.winners.into_iter().map(|p| lookup.player_id_to_name[&p.id].clone()).collect();
            let losers = result.losers.into_iter().map(|p| lookup.player_id_to_name[&p.id].clone()).collect();
            let pond_game_result = GameResult { winners, losers };
            let msg = ServerMessage::GameResult(pond_game_result);
            broadcast_server_message(msg, &comm.client_broadcast_tx);
            break;
        }
    }
}

async fn run_websocket(peer: SocketAddr, mut comm: WebsocketCommunicator, mut ws_stream: WebSocketStream<TcpStream>) {
    loop {
        tokio::select! {
            msg = comm.client_rx.recv() => {
                log::info!("Received client message: {:?}, sending on", msg);
                _ = ws_stream.send(msg.unwrap()).await;
            },
            msg = comm.client_broadcast_rx.recv() => { _ = ws_stream.send(msg.unwrap()).await; },
            msg = ws_stream.next() => {
                let m = msg.unwrap().unwrap();
                let msg = m.to_text().unwrap();
                log::info!("Received message: {:?} from {}", msg, peer);
                let msg: ClientMessage = serde_json::from_str(msg).unwrap();
                let msg: InternalClientMessage = InternalClientMessage { client: peer, message: msg };
                let json = serde_json::to_string(&msg).unwrap();
                _ = comm.hook_tx.send(Message::Text(json.into())).await; }
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let addr = "127.0.0.1:9001";
    let listener = TcpListener::bind(&addr).await.expect("Can't listen");
    info!("Listening on: {}", addr);

    let (hook_tx, hook_rx) = mpsc::channel::<Message>(10);
    let mut clients_tx: HashMap<PlayerId, mpsc::Sender<Message>> = HashMap::new();
    let (client_broadcast_tx, _) = broadcast::channel::<Message>(10);

    let mut names = vec!["alpha", "bravo"];
    let mut port_to_name: HashMap<u16, String> = HashMap::new();
    let mut name_to_player_id: HashMap<String, PlayerId> = HashMap::new();
    let mut player_id_to_name: HashMap<PlayerId, String> = HashMap::new();
    let mut pcount = 0;

    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        let name = names.pop().unwrap();
        let player_id = PlayerId(pcount);
        pcount += 1;
        info!("Client connected on: {}", peer);
        info!("Client name: {}, player id: {:?}", name, player_id);

        port_to_name.insert(peer.port(), name.to_string());
        name_to_player_id.insert(name.to_string(), player_id);
        player_id_to_name.insert(player_id, name.to_string());

        let (client_tx, client_rx) = mpsc::channel::<Message>(10);
        clients_tx.insert(player_id, client_tx);

        let websocket_communicator = WebsocketCommunicator {
            hook_tx: hook_tx.clone(),
            client_rx,
            client_broadcast_rx: client_broadcast_tx.subscribe()
        };

        tokio::spawn(accept_connection(peer, stream, websocket_communicator));

        if clients_tx.len() == 2 {
            break;
        }
    }

    let controller_communicator = ControllerCommunicator {
        hook_rx,
        clients_tx,
        client_broadcast_tx
    };

    let lookup = ControllerLookup {
        port_to_name,
        name_to_player_id,
        player_id_to_name
    };

    _ = tokio::spawn(run_controller(controller_communicator, lookup)).await;
}
