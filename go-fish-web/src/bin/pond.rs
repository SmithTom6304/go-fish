use futures_util::{SinkExt, StreamExt};
use go_fish::{Deck, Game, Hook, PlayerId};
use go_fish_web::{ClientHookRequest, ClientMessage, GameResult};
use go_fish_web::{FullHookRequest, HookAndResult, ServerMessage};
use go_fish_web::{PlayerState, PlayerTurnValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, tungstenite::{Error, Result}, WebSocketStream};
use tracing::{debug, error, info, trace};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

async fn accept_tcp_connection(peer: SocketAddr, stream: TcpStream, websocket_communicator: WebsocketCommunicator) {
    debug!(address = %peer, "Accepted tcp connection");
    if let Err(e) = handle_connection(peer, stream, websocket_communicator).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8(_) => (),
            err => error!(error = err.to_string(), "Error processing connection"),
        }
    }
}

async fn handle_connection(peer: SocketAddr, stream: TcpStream, websocket_communicator: WebsocketCommunicator) -> Result<()> {
    let ws_stream = accept_async(stream).await.expect("Failed to accept");
    debug!(address = %peer, "Accepted WebSocket connection");

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
    pub controller_tx: mpsc::Sender<Message>,
    pub controller_rx: mpsc::Receiver<Message>,
    pub controller_broadcast_rx: broadcast::Receiver<Message>,
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

async fn send_pregame_messages(game: &Game, comm: &mut ControllerCommunicator, lookup: &ControllerLookup) {
    debug!("Sending pregame messages to players");
    let current_player = game.get_current_player().expect("Current player should exist pregame");
    let current_player_name = lookup.player_id_to_name[&current_player.id].clone();
    for player in game.players.iter().clone() {
        let player_name = lookup.player_id_to_name[&player.id].clone();
        debug!(player_name, player_id = &player.id.0, "Sending pregame messages to player");

        let tx = &comm.clients_tx[&player.id];
        let state: PlayerState = PlayerState {
            hand: player.hand.clone(),
            completed_books: player.completed_books.clone(),
        };

        let msg = ServerMessage::PlayerIdentity(player_name);
        send_server_message(msg, tx).await;

        let msg = ServerMessage::PlayerState(state);
        send_server_message(msg, tx).await;

        let msg = match player.id == current_player.id {
            true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
            false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.clone())),
        };
        send_server_message(msg, tx).await;
    }
    debug!("Sent pregame messages to players");
}

async fn handle_client_hook_message(hook: ClientHookRequest,
                                    game: &mut Game,
                                    client_name: String,
                                    client_player_id: PlayerId,
                                    comm: &mut ControllerCommunicator,
                                    lookup: &ControllerLookup) {
    debug!(player_name = client_name,
                    player_id = &client_player_id.0,
                    hook.target_name,
                    %hook.rank,
                    "Handling client hook request");
    let current_player = game.get_current_player();
    let current_player = match current_player {
        Some(player) => player,
        None => {
            error!(player_name = client_name, player_id = &client_player_id.0, "No current player");
            return;
        }
    };
    if current_player.id != client_player_id {
        // TODO Not your turn!
        debug!(player_name = client_name, player_id = &client_player_id.0, "Player tried to go out of turn");
        return;
    };
    let target_name = hook.target_name;
    let target_player_id = lookup.name_to_player_id[&target_name];
    if current_player.id == target_player_id {
        // TODO Cannot target yourself!
        debug!(player_name = client_name, player_id = &client_player_id.0, "Player tried to target themselves");
        return;
    }

    if !current_player.hand.books.iter().any(|book| book.rank == hook.rank) {
        // TODO Cannot ask for a card you do not have!
        debug!(player_name = client_name,
                        player_id = &client_player_id.0,
                        rank = %hook.rank,
                        "Player tried to ask for a card they do not already have");
        return;
    }

    let rank = hook.rank;
    let hook = Hook { target: target_player_id, rank: hook.rank };
    let result = game.take_turn(hook).unwrap();

    let full_request = FullHookRequest {
        fisher_name: client_name,
        target_name,
        rank,
    };

    let hook_result_message = ServerMessage::HookAndResult(HookAndResult { hook_request: full_request, hook_result: result });
    broadcast_server_message(hook_result_message, &comm.client_broadcast_tx);

    if game.is_finished {
        return;
    }

    let current_player = game.get_current_player().expect("Current player should exist ingame");
    let current_player_name = lookup.player_id_to_name[&current_player.id].clone();

    for player in game.players.iter().clone() {
        debug!(player_id = player.id.0, "Sending client state");
        let tx = &comm.clients_tx[&player.id];
        let state: PlayerState = PlayerState {
            hand: player.hand.clone(),
            completed_books: player.completed_books.clone(),
        };

        let msg = ServerMessage::PlayerState(state);
        send_server_message(msg, tx).await;

        let msg = match player.id == current_player.id {
            true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
            false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.clone())),
        };
        send_server_message(msg, tx).await;
    }
}

async fn run_controller(mut comm: ControllerCommunicator, lookup: ControllerLookup) {
    debug!("Running controller handler");
    let mut deck = Deck::new();
    deck.shuffle();
    let mut game = Game::new(deck, 2);

    send_pregame_messages(&game, &mut comm, &lookup).await;

    while let Some(msg) = comm.hook_rx.recv().await {
        debug!(%msg, "Received internal client message");
        let icm = msg.to_text().unwrap();
        let icm = serde_json::from_str::<InternalClientMessage>(icm).unwrap();

        let client_name = lookup.port_to_name[&icm.client.port()].clone();
        let client_player_id = lookup.name_to_player_id[&client_name];

        match icm.message {
            ClientMessage::Hook(hook) => {
                handle_client_hook_message(hook, &mut game, client_name, client_player_id, &mut comm, &lookup).await;
                if game.is_finished {
                    info!("Game finished!");
                    let result = game.get_game_result().unwrap();
                    let winners = result.winners.into_iter().map(|p| lookup.player_id_to_name[&p.id].clone()).collect();
                    let losers = result.losers.into_iter().map(|p| lookup.player_id_to_name[&p.id].clone()).collect();
                    let pond_game_result = GameResult { winners, losers };
                    let msg = ServerMessage::GameResult(pond_game_result);
                    debug!(message = ?msg, "Broadcasting game finished result");
                    broadcast_server_message(msg, &comm.client_broadcast_tx);
                    break;
                }
            }
            ClientMessage::Disconnect => {
                debug!("Sending Close message to all clients");
                for tx in &comm.clients_tx {
                    let message = Message::Close(None);
                    _ = tx.1.send(message).await;
                }
                continue;
            }
        }
    }
    debug!("Closing controller handler");
}

async fn run_websocket(peer: SocketAddr, mut comm: WebsocketCommunicator, mut ws_stream: WebSocketStream<TcpStream>) {
    debug!(client_address = %peer, "Running websocket handler");
    loop {
        tokio::select! {
            msg = comm.controller_rx.recv() => {
                let message = match msg {
                    Some(msg) => msg,
                    None => {
                        debug!(client_address = %peer, "Controller has closed the internal connection, closing websocket");
                        break;
                    },
                };
                trace!(client_address = %peer, %message, "Received message from controller, sending to client");
                _ = ws_stream.send(message).await;
            },
            msg = comm.controller_broadcast_rx.recv() => {
                let message = match msg {
                    Ok(msg) => msg,
                    Err(err) => {
                        match err {
                            RecvError::Closed => {
                                debug!(client_address = %peer, "Controller has closed the internal broadcast connection, closing websocket");
                                break;
                            },
                            RecvError::Lagged(_) => {
                                error!(client_address = %peer, error = %err, "Error receiving internal broadcast message from controller");
                                continue;
                            }
                        }
                    },
                };
                trace!(client_address = %peer, %message, "Received broadcast message from controller, sending to client");
                _ = ws_stream.send(message).await;
            },
            msg = ws_stream.next() => {
                debug!(client_address = %peer, message = ?msg, "Received message from websocket");
                let message = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(err)) => {
                        error!(client_address = %peer, error = %err, "Error receiving message from client");
                        continue;
                    },
                    None => {
                        debug!(client_address = %peer, "Client has force closed the websocket connection");
                        // let msg: ClientMessage = ClientMessage::Disconnect;
                        // let msg: InternalClientMessage = InternalClientMessage { client: peer, message: msg };
                        // let json = serde_json::to_string(&msg).unwrap();
                        // _ = comm.controller_tx.send(Message::Text(json.into())).await;
                        break;
                    }
                };
                trace!(client_address = %peer, %message, "Received message from client, sending to controller");
                let message = match message {
                    Message::Close(_close_frame) => {
                        debug!(client_address = %peer, "Client has closed the websocket connection");
                        let msg: ClientMessage = ClientMessage::Disconnect;
                        let msg: InternalClientMessage = InternalClientMessage { client: peer, message: msg };
                        let json = serde_json::to_string(&msg).unwrap();
                        _ = comm.controller_tx.send(Message::Text(json.into())).await;
                        continue;
                    },
                    Message::Text(text) => {
                        serde_json::from_str(&text).unwrap()
                    },
                    _ => todo!()
                };
                let msg: InternalClientMessage = InternalClientMessage { client: peer, message };
                let json = serde_json::to_string(&msg).unwrap();
                _ = comm.controller_tx.send(Message::Text(json.into())).await; }
        }
    }
    debug!(client_address = %peer, "Closing websocket handler");
}

fn init_logging() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}

#[tokio::main]
async fn main() {
    init_logging();
    let address = "127.0.0.1:9001";

    info!(address, "Starting pond");

    let listener = TcpListener::bind(&address).await.expect("Can't listen");

    let (hook_tx, hook_rx) = mpsc::channel::<Message>(10);
    let mut clients_tx: HashMap<PlayerId, mpsc::Sender<Message>> = HashMap::new();
    let (client_broadcast_tx, _) = broadcast::channel::<Message>(10);

    let mut names = vec!["alpha", "bravo"];
    let mut port_to_name: HashMap<u16, String> = HashMap::new();
    let mut name_to_player_id: HashMap<String, PlayerId> = HashMap::new();
    let mut player_id_to_name: HashMap<PlayerId, String> = HashMap::new();
    let mut pcount = 0;
    let max_clients = 2;

    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        info!(client_address = %peer, "Client connected");

        let player_name = names.pop().unwrap();
        let player_id = PlayerId(pcount);
        pcount += 1;
        debug!(client_address = %peer, player_name, player_id = player_id.0, "New player mapped to client");

        port_to_name.insert(peer.port(), player_name.to_string());
        name_to_player_id.insert(player_name.to_string(), player_id);
        player_id_to_name.insert(player_id, player_name.to_string());

        let (client_tx, client_rx) = mpsc::channel::<Message>(10);
        clients_tx.insert(player_id, client_tx);

        let websocket_communicator = WebsocketCommunicator {
            controller_tx: hook_tx.clone(),
            controller_rx: client_rx,
            controller_broadcast_rx: client_broadcast_tx.subscribe()
        };

        tokio::spawn(accept_tcp_connection(peer, stream, websocket_communicator));

        if clients_tx.len() == max_clients {
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

    drop(hook_tx);

    info!(max_clients, "Max clients connected. Starting game.");
    _ = tokio::spawn(run_controller(controller_communicator, lookup)).await;
}
