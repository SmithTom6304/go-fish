use anyhow::anyhow;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use go_fish::{Deck, Game, Hand, Hook, Player, PlayerId, Rank};
use go_fish_game_server_prototype::{AddressedClientMessage, Config};
use go_fish_web::HookError::{CannotTargetYourself, NotYourTurn, YouDoNotHaveRank};
use go_fish_web::{ClientHookRequest, ClientMessage, GameResult, HookError};
use go_fish_web::{FullHookRequest, HookAndResult, ServerMessage};
use go_fish_web::{PlayerState, PlayerTurnValue};
use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::{accept_async, tungstenite::{Error, Result}, WebSocketStream};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tungstenite::Message;

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

    let controller_rx = websocket_communicator.server_message_rx;
    let controller_tx = websocket_communicator.client_message_tx.clone();
    run_websocket(peer, ws_stream, controller_rx, controller_tx, handle_server_message, handle_websocket_message).await;

    Ok(())
}

struct ControllerCommunicator {
    pub client_message_rx: mpsc::Receiver<AddressedClientMessage>,
    pub client_server_messages_tx: HashMap<PlayerId, mpsc::Sender<ServerMessage>>
}

struct ControllerLookup {
    pub client_address_to_name: HashMap<SocketAddr, String>,
    pub name_to_player_id: HashMap<String, PlayerId>,
    pub player_id_to_name: HashMap<PlayerId, String>,
}

struct WebsocketCommunicator {
    pub client_message_tx: mpsc::Sender<AddressedClientMessage>,
    pub server_message_rx: mpsc::Receiver<ServerMessage>,
}

async fn send_pregame_messages(game: &Game, comm: &mut ControllerCommunicator, lookup: &ControllerLookup) {
    debug!("Sending pregame messages to players");
    let current_player = game.get_current_player().expect("Current player should exist pregame");
    let current_player_name = lookup.player_id_to_name[&current_player.id].clone();
    for player in game.players.iter().clone() {
        let player_name = lookup.player_id_to_name[&player.id].clone();
        debug!(player_name, player_id = &player.id.0, "Sending pregame messages to player");

        let tx = &comm.client_server_messages_tx[&player.id];
        let state: PlayerState = PlayerState {
            hand: player.hand.clone(),
            completed_books: player.completed_books.clone(),
        };

        let msg = ServerMessage::PlayerIdentity(player_name);
        _ = tx.send(msg).await;

        let msg = ServerMessage::PlayerState(state);
        _ = tx.send(msg).await;

        let msg = match player.id == current_player.id {
            true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
            false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.clone())),
        };
        _ = tx.send(msg).await;
    }
    debug!("Sent pregame messages to players");
}

async fn handle_client_hook_message(hook_request: ClientHookRequest,
                                    game: &mut Game,
                                    client_name: String,
                                    client_player_id: PlayerId,
                                    comm: &mut ControllerCommunicator,
                                    lookup: &ControllerLookup)
{
    debug!(player_name = client_name,
                    player_id = &client_player_id.0,
                    hook_request.target_name,
                    %hook_request.rank,
                    "Handling client hook request");
    let current_player = game.get_current_player();
    let current_player = match current_player {
        Some(player) => player,
        None => {
            error!(player_name = client_name, player_id = &client_player_id.0, "No current player");
            return;
        }
    };
    let client_tx = comm.client_server_messages_tx.get(&client_player_id).unwrap();

    if let Some(hook_error) = not_your_turn_hook_guard(&client_player_id, &current_player.id) {
        let message = ServerMessage::HookError(hook_error);
        _ = client_tx.send(message).await;
        return;
    }

    let target_player_id = match find_hook_target(&hook_request, &lookup.name_to_player_id) {
        FindHookTargetResult::Found(target_player_id) => target_player_id,
        FindHookTargetResult::UnknownPlayer(target_name) => {
            let message = ServerMessage::HookError(go_fish_web::HookError::UnknownPlayer(target_name));
            _ = client_tx.send(message).await;
            return;
        }
    };

    if let Some(hook_error) = cannot_target_self_hook_guard(current_player.id, target_player_id) {
        let message = ServerMessage::HookError(hook_error);
        _ = client_tx.send(message).await;
        return;
    }

    if let Some(hook_error) = do_not_have_rank_hook_guard(current_player.hand.clone(), hook_request.rank) {
        let message = ServerMessage::HookError(hook_error);
        _ = client_tx.send(message).await;
        return;
    }

    let rank = hook_request.rank;
    let hook = Hook { target: target_player_id, rank: hook_request.rank };
    let result = game.take_turn(hook).unwrap();

    let full_request = FullHookRequest {
        fisher_name: client_name,
        target_name: hook_request.target_name,
        rank,
    };

    let hook_result_message = ServerMessage::HookAndResult(HookAndResult { hook_request: full_request, hook_result: result });
    for tx in comm.client_server_messages_tx.values() {
        _ = tx.send(hook_result_message.clone()).await;
    }

    if game.is_finished {
        return;
    }

    let current_player = game.get_current_player().expect("Current player should exist ingame");
    let current_player_name = lookup.player_id_to_name[&current_player.id].clone();

    for player in game.players.iter().clone() {
        debug!(player_id = player.id.0, "Sending player state");
        let tx = &comm.client_server_messages_tx[&player.id];
        for msg in create_player_state_server_messages(player, current_player.id, &current_player_name) {
            _ = tx.send(msg).await;
        }
    }
}

fn create_player_state_server_messages(player: &Player, current_player_id: PlayerId, current_player_name: &str) -> [ServerMessage; 2] {
    let state: PlayerState = PlayerState {
        hand: player.hand.clone(),
        completed_books: player.completed_books.clone(),
    };

    let state_msg = ServerMessage::PlayerState(state);

    let turn_msg = match player.id == current_player_id {
        true => ServerMessage::PlayerTurn(PlayerTurnValue::YourTurn),
        false => ServerMessage::PlayerTurn(PlayerTurnValue::OtherTurn(current_player_name.to_string())),
    };

    [state_msg, turn_msg]
}

async fn handle_player_name_change_request(client: &SocketAddr, name_request: String, comm: &mut ControllerCommunicator, lookup: &mut ControllerLookup) {
    let existing_client_using_name = lookup.client_address_to_name.iter()
        .find(|(_, name)| *name == &name_request)
        .map(|(address, _)| address);
    if let Some(existing_port_using_name) = existing_client_using_name {
        debug!(player_name = name_request, %existing_port_using_name, "Player name already in use");
        return;
    }

    let old_name = lookup.client_address_to_name.get(client);
    let old_name = match old_name {
        Some(old_name) => old_name.clone(),
        None => {
            warn!("Client does not exist in client to name lookup");
            return;
        }
    };

    update_name_lookups(client, old_name.clone(), name_request.clone(), lookup).unwrap();
    debug!(old_name, name_request, %client, "Successfully updated name lookups for client");
    let msg = ServerMessage::PlayerIdentity(name_request.clone());
    let tx = &comm.client_server_messages_tx[&lookup.name_to_player_id[&name_request]];
    _ = tx.send(msg).await;
}

fn update_name_lookups(client: &SocketAddr, old_name: String, new_name: String, lookup: &mut ControllerLookup) -> Result<(), anyhow::Error> {
    debug!(old_name, new_name, %client, "Updating name lookups for client");

    // Client address to name
    if let Some(old_name) = lookup.client_address_to_name.get_mut(client) {
        *old_name = new_name.clone();
    } else {
        error!(%client, "Could not find client in port to name lookup");
        return Err(anyhow!("Could not find client in port to name lookup"));
    }

    // Name to player id
    let player_id = lookup.name_to_player_id.remove(&old_name);
    let player_id = match player_id {
        None => {
            error!(old_name, "Could not find old name in name to player id lookup");
            return Err(anyhow!("Could not find old name in name to player id lookup"))
        },
        Some(player_id) => player_id
    };
    lookup.name_to_player_id.insert(new_name.clone(), player_id);

    // Player id to name
    if let Some(old_name) = lookup.player_id_to_name.get_mut(&player_id) {
        *old_name = new_name;
    } else {
        error!(%client, "Could not find player id in player id to name lookup");
        return Err(anyhow!("Could not find player id in player id to name lookup"));
    }

    Ok(())
}

fn handle_server_message(message: ServerMessage) -> Message {
    match message {
        ServerMessage::Disconnect => Message::Close(None),
        _ => Message::Text(serde_json::to_string(&message).unwrap().into())
    }
}

fn handle_websocket_message(message: Message) -> Result<ClientMessage, anyhow::Error> {
    match message {
        Message::Text(text) => Ok(serde_json::from_str(&text)?),
        Message::Close(_) => Ok(ClientMessage::Disconnect),
        _ => Err(anyhow!("Invalid message type")),
    }
}

fn not_your_turn_hook_guard(client_player_id: &PlayerId,
                            current_player_id: &PlayerId,
) -> Option<HookError> {
    match current_player_id.0 == client_player_id.0 {
        true => None,
        false => Some(NotYourTurn)
    }
}

enum FindHookTargetResult {
    Found(PlayerId),
    UnknownPlayer(String),
}

fn find_hook_target(hook_request: &ClientHookRequest,
                    name_id_map: &HashMap<String, PlayerId>,
) -> FindHookTargetResult {
    let target_name = hook_request.target_name.clone();
    let target_player_id = name_id_map.get(&target_name);
    match target_player_id {
        Some(id) => FindHookTargetResult::Found(*id),
        None => FindHookTargetResult::UnknownPlayer(target_name),
    }
}

fn cannot_target_self_hook_guard(current_player_id: PlayerId,
                                 target_player_id: PlayerId,
) -> Option<HookError> {
    match current_player_id.0 == target_player_id.0 {
        true => Some(CannotTargetYourself),
        false => None,
    }
}

fn do_not_have_rank_hook_guard(current_player_hand: Hand,
                               rank: Rank,
) -> Option<HookError> {
    match current_player_hand.books.iter().any(|book| book.rank == rank) {
        true => None,
        false => Some(YouDoNotHaveRank(rank))
    }
}

async fn run_controller(mut comm: ControllerCommunicator, mut lookup: ControllerLookup) {
    debug!("Running controller handler");
    let mut deck = Deck::new();
    deck.shuffle();
    let mut game = Game::new(deck, 2);

    send_pregame_messages(&game, &mut comm, &lookup).await;

    while let Some(msg) = comm.client_message_rx.recv().await {
        debug!(?msg, "Received internal client message");
        let client_name = lookup.client_address_to_name[&msg.client].clone();
        let client_player_id = lookup.name_to_player_id[&client_name];

        match msg.client_message {
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
                    for tx in comm.client_server_messages_tx.values() {
                        _ = tx.send(msg.clone()).await;
                    }
                    break;
                }
            },
            ClientMessage::PlayerNameChangeRequest(name_request) => {
                debug!(player_name = client_name, new_name = name_request, "Handling player name change request");
                handle_player_name_change_request(&msg.client, name_request, &mut comm, &mut lookup).await;
            }
            ClientMessage::Disconnect => {
                debug!("Sending Close message to all clients");
                for tx in &comm.client_server_messages_tx {
                    _ = tx.1.send(ServerMessage::Disconnect).await;
                }
                continue;
            }
        }
    }
    debug!("Closing controller handler");
}

async fn run_websocket<F, G>(peer: SocketAddr,
                             mut ws_stream: WebSocketStream<TcpStream>,
                             mut controller_rx: Receiver<ServerMessage>,
                             controller_tx: Sender<AddressedClientMessage>,
                             handle_server_message: F,
                             handle_websocket_message: G)
where
    F: Fn(ServerMessage) -> Message,
    G: Fn(Message) -> Result<ClientMessage, anyhow::Error>,
{
    debug!(client_address = %peer, "Running websocket handler");
    loop {
        tokio::select! {
            msg = controller_rx.recv() => {
                let message = match msg {
                    Some(msg) => msg,
                    None => {
                        debug!(client_address = %peer, "Controller has closed the internal connection, closing websocket");
                        break;
                    },
                };
                trace!(client_address = %peer, ?message, "Received message from controller, sending to client");
                let ws_message = handle_server_message(message);
                _ = ws_stream.send(ws_message).await;
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
                        break;
                    }
                };
                let client_message = handle_websocket_message(message);
                let client_message = match client_message {
                    Ok(client_message) => client_message,
                    Err(err) => {
                        error!(client_address = %peer, error = %err, "Error handling websocket message");
                        continue;
                    }
                };
                let addressed_client_message = AddressedClientMessage {
                    client: peer,
                    client_message
                };
                _ = controller_tx.send(addressed_client_message).await;
            }
        }
    }
    debug!(client_address = %peer, "Closing websocket handler");
}

fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(env_filter)
        .init();
}

fn load_config(path: &Path) -> Result<Config, anyhow::Error> {
    if !path.exists() {
        return Err(anyhow!("Config file '{}' does not exist", path.display()));
    }
    let config = fs::read_to_string(path)?;
    let config = toml::from_str(&config)?;
    Ok(config)
}

#[tokio::main]
async fn main() {
    init_logging();
    let args = dbg!(Args::parse());
    let config = match load_config(&args.config) {
        Ok(config) => config,
        Err(err) => {
            warn!(error = %err, "Error loading config. Using defaults");
            Config::default()
        }
    };

    info!(?args, ?config, "Starting go-fish-game-server");

    let listener = TcpListener::bind(&config.address).await.expect("Can't listen");

    let (client_message_tx, client_message_rx) = mpsc::channel::<AddressedClientMessage>(10);
    let mut clients_tx: HashMap<PlayerId, mpsc::Sender<ServerMessage>> = HashMap::new();

    let mut names = vec!["alpha", "bravo", "charlie"];
    let mut client_address_to_name: HashMap<SocketAddr, String> = HashMap::new();
    let mut name_to_player_id: HashMap<String, PlayerId> = HashMap::new();
    let mut player_id_to_name: HashMap<PlayerId, String> = HashMap::new();
    let mut pcount = 0;

    while let Ok((stream, _)) = listener.accept().await {
        let peer = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        info!(client_address = %peer, "Client connected");

        let player_name = names.pop().unwrap();
        let player_id = PlayerId(pcount);
        pcount += 1;
        debug!(client_address = %peer, player_name, player_id = player_id.0, "New player mapped to client");

        client_address_to_name.insert(peer, player_name.to_string());
        name_to_player_id.insert(player_name.to_string(), player_id);
        player_id_to_name.insert(player_id, player_name.to_string());

        let (server_message_tx, server_message_rx) = mpsc::channel::<ServerMessage>(10);
        clients_tx.insert(player_id, server_message_tx);

        let websocket_communicator = WebsocketCommunicator {
            client_message_tx: client_message_tx.clone(),
            server_message_rx
        };

        tokio::spawn(accept_tcp_connection(peer, stream, websocket_communicator));

        if clients_tx.len() == config.player_count {
            break;
        }
    }

    let controller_communicator = ControllerCommunicator {
        client_message_rx,
        client_server_messages_tx: clients_tx
    };

    let lookup = ControllerLookup {
        client_address_to_name,
        name_to_player_id,
        player_id_to_name
    };

    drop(client_message_tx);

    info!(config.player_count, "Max clients connected. Starting game.");
    _ = tokio::spawn(run_controller(controller_communicator, lookup)).await;
}

/// go-fish game server
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    pub config: PathBuf,
}
