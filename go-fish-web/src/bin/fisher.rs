use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use go_fish::{HookResult, Rank};
use go_fish_web::GameResult;
use go_fish_web::{ClientHookRequest, ClientMessage, HookAndResult, PlayerState, PlayerTurnValue};
use std::io::stdin;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug)]
enum IoMessage {
    ServerMessage(go_fish_web::ServerMessage),
    Close,
}

fn parse_client_message_from_string(s: &str) -> anyhow::Result<ClientMessage> {
    debug!(value = s, "Parsing client request");
    let parts = s.split(' ').map(|part| part.trim().to_lowercase()).collect::<Vec<String>>();
    debug!(value = ?parts, "Parsed client request");
    if parts.len() == 1 && parts[0] == "exit" {
        debug!("Parsed disconnect message");
        return Ok(ClientMessage::Disconnect)
    }

    if parts.len() != 2 {
        return Err(anyhow::anyhow!("Invalid hook request"));
    }

    let target_name = parts[0].clone();
    let rank = parts[1].clone();
    debug!(target_name, rank, "Parsed hook message");
    let rank = match rank.as_str() {
        "ace" => Rank::Ace,
        "king" => Rank::King,
        "queen" => Rank::Queen,
        "jack" => Rank::Jack,
        "ten" => Rank::Ten,
        "nine" => Rank::Nine,
        "eight" => Rank::Eight,
        "seven" => Rank::Seven,
        "six" => Rank::Six,
        "five" => Rank::Five,
        "four" => Rank::Four,
        "three" => Rank::Three,
        "two" => Rank::Two,
        _ => return Err(anyhow::anyhow!("Invalid hook request rank")),
    };

    Ok(ClientMessage::Hook(ClientHookRequest { target_name: target_name.to_string(), rank }))
}

fn handle_server_message(message: go_fish_web::ServerMessage) -> anyhow::Result<()> {
    match message {
        go_fish_web::ServerMessage::HookAndResult(hook_and_result) => {
            debug!(?hook_and_result, "Received hook and result");
            handle_hook_and_result(hook_and_result)
        }
        go_fish_web::ServerMessage::PlayerState(state) => {
            debug!(?state, "Received player state");
            handle_player_state(state)
        },
        go_fish_web::ServerMessage::PlayerTurn(player_turn) => {
            debug!(?player_turn, "Received player turn");
            handle_player_turn(player_turn)
        },
        go_fish_web::ServerMessage::PlayerIdentity(identity) => {
            debug!(player_identity = ?identity, "Received player identity");
            handle_player_identity(identity)
        },
        go_fish_web::ServerMessage::GameResult(game_result) => {
            debug!(?game_result, "Received game result");
            handle_game_result(game_result)
        }
    }
    Ok(())
}

fn handle_hook_and_result(hook_and_result: HookAndResult) {
    let result = match hook_and_result.hook_result {
        HookResult::Catch(catch) => format!("caught {} {}s!", catch.cards.len(), catch.rank.to_string().to_lowercase()),
        HookResult::GoFish => "go fish!".to_string(),
    };
    println!("{} asked {} for {}s - {}",
             hook_and_result.hook_request.fisher_name,
             hook_and_result.hook_request.target_name,
             hook_and_result.hook_request.rank,
             result
    );
}

fn handle_player_state(player_state: PlayerState) {
    println!("Completed books: {}", player_state.completed_books.iter().map(|b| b.rank.to_string()).collect::<Vec<_>>().join(", "));
    println!("Incomplete books: {}",
        player_state.hand.books.iter().map(|b| format!("{} {}s", b.cards.len(), b.rank)).collect::<Vec<_>>().join(", ")
    );
}

fn handle_player_turn(player_turn: PlayerTurnValue) {
    match player_turn {
        PlayerTurnValue::YourTurn => println!("It is your turn!"),
        PlayerTurnValue::OtherTurn(name) => println!("It is {}s turn", name)
    }
}

fn handle_player_identity(name: String) {
    println!("You are player {}", name);
}

fn handle_game_result(result: GameResult) {
    println!("Game finished!");
    println!("Winners: {}", result.winners.join(", "));
}

fn parse_user_input(input: String) -> Result<ClientMessage, anyhow::Error> {
    debug!(input, "Handling user input");
    let req = parse_client_message_from_string(&input);
    match req {
        Ok(req) => Ok(req),
        Err(err) => Err(anyhow::anyhow!(err))
    }
}

fn run_user_input(input_tx: mpsc::Sender<String>) {
    debug!("Running user input handler");
    loop {
        let mut s = String::new();
        _ = stdin().read_line(&mut s);
        debug!(value = s, "Received user input");
        input_tx.blocking_send(s).unwrap()
    }
}

async fn run_output(mut io_rx: mpsc::Receiver<IoMessage>, cancel: CancellationToken) {
    debug!("Running output handler");
    loop {
        let message = tokio::select! {
            message = io_rx.recv() => message,
            _ = cancel.cancelled() => break,
        };
        debug!(?message, "Received io message");
        match message {
            None => {
                info!("IO connection closed");
                return;
            },
            Some(IoMessage::ServerMessage(message)) => {
                debug!(message = ?message, "io Received server message");
                handle_server_message(message).unwrap();
            },
            Some(IoMessage::Close) => {
                info!("Closing IO connection");
                return;
            }
        }
    }
}

async fn run_websocket(mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
                       mut user_input_rx: mpsc::Receiver<String>,
                       io_tx: mpsc::Sender<IoMessage>,
                       cancel: CancellationToken) {
    debug!("Running websocket handler");
    loop {
        tokio::select! {
            msg = ws.next() => {
                let message = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(err)) => {
                        error!(error = %err, "Error receiving message from server");
                        continue;
                    },
                    None => {
                        info!("Server has closed the websocket connection");
                        let message = IoMessage::Close;
                        io_tx.send(message).await.unwrap();
                        break;
                    }
                };
                debug!(%message, "Received server message");
                let message = match message {
                    Message::Close(_close_frame) => {
                        debug!("Received close frame from server");
                        continue;
                    },
                    Message::Text(_) => message,
                    _ => todo!()
                };
                let message = message.into_text().unwrap();
                let message: go_fish_web::ServerMessage = serde_json::from_str(&message).unwrap();
                let message = IoMessage::ServerMessage(message);
                io_tx.send(message).await.unwrap();
                debug!("Sent io message");
            },
            msg = user_input_rx.recv() => {
                let msg = msg.expect("user input connection should never close");
                debug!(message = %msg, "Received user input message");
                let message = parse_user_input(msg);
                match message {
                    Ok(message) => {
                        let json = serde_json::to_string(&message).unwrap();
                        let message = Message::Text(json.into());
                        ws.send(message).await.unwrap();
                    },
                    Err(err) => {
                        error!(error = %err, "Error receiving user input");
                    }
                }
            },
            _ = cancel.cancelled() => break
        }
    }
    debug!("Closing websocket handler");
}

fn init_logging() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}

async fn run() {
    let server_address = "ws://localhost:9001";
    let (socket, _) = connect_async(server_address).await.unwrap();
    let(user_input_tx, user_input_rx) = mpsc::channel::<String>(10);
    let (internal_tx, internal_rx) = mpsc::channel::<IoMessage>(10);
    info!(server_address, "Connected to server");

    let cancel = CancellationToken::new();

    _ = tokio::task::spawn_blocking(|| {run_user_input(user_input_tx)});
    _ = tokio::spawn(run_output(internal_rx, cancel.clone()));
    _ = tokio::spawn(run_websocket(socket, user_input_rx, internal_tx, cancel.clone())).await;

    cancel.cancel();
}

fn main() {
    init_logging();
    info!("Starting fisher");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        run().await;
    });
    
    info!("Closing fisher");
    runtime.shutdown_timeout(Duration::from_millis(500));
}