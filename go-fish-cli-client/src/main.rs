use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use futures_util::{SinkExt, StreamExt};
use go_fish::{HookResult, Rank};
use go_fish_web::{ClientHookRequest, ClientMessage, HookAndResult, PlayerState, PlayerTurnValue};
use go_fish_web::{GameResult, HookError};
use std::io::stdin;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser, Debug)]
#[command(long_about = None, name = "game")]
struct GameArgs {
    #[command(subcommand)]
    command: GameCommand,
}

#[derive(Debug, Subcommand)]
enum GameCommand {
    /// Try fish a card from a player
    Hook {
        /// The name of the player to fish from
        name: String,
        /// The rank of the card to fish. You must have this in your hand
        rank: String,
    },
    /// Change your name
    Name {
        new_name: String
    },
    /// Exit the game
    Exit,
}

impl TryFrom<GameCommand> for ClientMessage {
    type Error = anyhow::Error;
    fn try_from(command: GameCommand) -> Result<ClientMessage, Self::Error> {
        let message = match command {
            GameCommand::Hook { name, rank } => {
                let rank = try_parse_rank_from_string(&rank)?;
                ClientMessage::Hook(ClientHookRequest { target_name: name, rank })
            }
            GameCommand::Name { new_name } => ClientMessage::PlayerNameChangeRequest(new_name),
            GameCommand::Exit => ClientMessage::Disconnect
        };
        Ok(message)
    }
}

fn try_parse_rank_from_string(rank: &str) -> Result<Rank, anyhow::Error> {
    let rank = match rank {
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
    Ok(rank)
}

#[derive(Debug)]
enum IoMessage {
    ServerMessage(go_fish_web::ServerMessage),
    Close,
}

fn handle_server_message(message: go_fish_web::ServerMessage) {
    match message {
        go_fish_web::ServerMessage::HookAndResult(hook_and_result) => {
            debug!(?hook_and_result, "Received hook and result");
            handle_hook_and_result(hook_and_result)
        },
        go_fish_web::ServerMessage::HookError(hook_error) => {
            debug!(?hook_error, "Received hook error");
            handle_hook_error(hook_error)
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
        },
        go_fish_web::ServerMessage::Disconnect => {}
    }
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

fn handle_hook_error(hook_error: HookError) {
    let message = match hook_error {
        HookError::NotYourTurn => "it is not your turn".to_string(),
        HookError::UnknownPlayer(player) => format!("unknown player {}", player),
        HookError::CannotTargetYourself => "cannot target yourself".to_string(),
        HookError::YouDoNotHaveRank(_) => "you cannot fish for a rank you do not have".to_string(),
    };

    println!("Invalid hook - {}", message);
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

fn run_user_input(input_tx: mpsc::Sender<GameCommand>) {
    debug!("Running user input handler");
    let mut args_to_display = GameArgs::command()
        .override_usage(None)
        .about(None);
    _ = args_to_display.print_help();
    loop {
        let mut s = String::new();
        _ = stdin().read_line(&mut s);
        s = format! {"game {}", s};
        let split = s.split(' ').map(|part| part.trim().to_lowercase());
        let game_args = GameArgs::try_parse_from(split);

        match game_args {
            Ok(args) => {
                debug!(value = ?args, "Received user input");
                input_tx.blocking_send(args.command).unwrap()
            },
            Err(err) => {
                error!(error = %err, "Error parsing game args");
            }
        }
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
                handle_server_message(message);
            },
            Some(IoMessage::Close) => {
                info!("Closing IO connection");
                return;
            }
        }
    }
}

async fn run_websocket(mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
                       mut user_input_rx: mpsc::Receiver<GameCommand>,
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
                debug!(message = ?msg, "Received user input message");
                let message = ClientMessage::try_from(msg);
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
    let (user_input_tx, user_input_rx) = mpsc::channel::<GameCommand>(10);
    let (internal_tx, internal_rx) = mpsc::channel::<IoMessage>(10);
    info!(server_address, "Connected to server");

    let cancel = CancellationToken::new();

    _ = tokio::task::spawn_blocking(|| {run_user_input(user_input_tx)});
    _ = tokio::spawn(run_websocket(socket, user_input_rx, internal_tx, cancel.clone()));
    _ = tokio::spawn(run_output(internal_rx, cancel.clone())).await;

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