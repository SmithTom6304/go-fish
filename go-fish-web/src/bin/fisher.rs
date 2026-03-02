use futures_util::{SinkExt, StreamExt};
use go_fish::{HookResult, Rank};
use go_fish_web::GameResult;
use go_fish_web::{ClientHookRequest, ClientMessage, HookAndResult, PlayerState, PlayerTurnValue, ServerMessage};
use std::io::stdin;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn parse_hook_request(s: &str) -> anyhow::Result<ClientHookRequest> {
    debug!(value = s, "Parsing hook request");
    let parts = s.split(' ').collect::<Vec<&str>>();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!("Invalid hook request"));
    }

    let target_name = parts[0].trim().to_string();
    let rank = parts[1].trim().to_lowercase().to_string();
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

    Ok(ClientHookRequest{target_name: target_name.to_string(), rank})
}

fn handle_server_message(message: Message) -> anyhow::Result<()> {
    let bytes = message.into_text()?;
    let server_message = bytes.as_str();
    let server_message = serde_json::from_str::<ServerMessage>(server_message)?;
    match server_message {
        ServerMessage::HookAndResult(hook_and_result) => {
            debug!(?hook_and_result, "Received hook and result");
            handle_hook_and_result(hook_and_result)
        }
        ServerMessage::PlayerState(state) => {
            debug!(?state, "Received player state");
            handle_player_state(state)
        },
        ServerMessage::PlayerTurn(player_turn) => {
            debug!(?player_turn, "Received player turn");
            handle_player_turn(player_turn)
        },
        ServerMessage::PlayerIdentity(identity) => {
            debug!(player_identity = ?identity, "Received player identity");
            handle_player_identity(identity)
        },
        ServerMessage::GameResult(game_result) => {
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

async fn run_io(io_tx: mpsc::Sender<Message>) {
    debug!("Running io handler");
    loop {
        let read_io_task = task::spawn_blocking(|| {
            let mut s = String::new();
            _ = stdin().read_line(&mut s);
            s
        });
        let text = read_io_task.await.unwrap();
        let req = parse_hook_request(&text);
        match req {
            Ok(req) => {
                let client_req = ClientMessage::Hook(req);
                debug!(message = ?client_req, "Sending io client message");
                io_tx.send(Message::text::<String>(serde_json::to_string(&client_req).unwrap())).await.unwrap()
            },
            Err(err) => error!(error = %err, "Error parsing hook request")
        }
    }
}

async fn run_websocket(mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>, mut io_rx: mpsc::Receiver<Message>) {
    debug!("Running websocket handler");
    loop {
        tokio::select! {
                msg = ws.next() => {
                    let server_message = msg.unwrap().unwrap();
                    debug!(%server_message, "Received server message");
                    match handle_server_message(server_message.clone()) {
                    Ok(()) => debug!(%server_message, "Handled server message successfully"),
                    Err(err) => error!(%server_message, "Error handling server message")
                }},
                msg = io_rx.recv() => {
                    let msg = msg.unwrap();
                    debug!(message = %msg, "Received io message");
                    ws.send(msg).await.unwrap();
                }
            }
    }
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
    info!("Starting fisher");

    let server_address = "ws://localhost:9001";
    let (socket, _) = connect_async(server_address).await.unwrap();
    let(io_tx, io_rx) = mpsc::channel::<Message>(10);
    info!(server_address, "Connected to server");

    _ = tokio::spawn(run_io(io_tx));
    _ = tokio::spawn(run_websocket(socket, io_rx)).await;
}